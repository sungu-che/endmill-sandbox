//! Vulkan 백엔드 검증 (feature = "vulkan")
//!
//! endmill 모델(SigLIP2 · ModernBERT/laya · TTM)이 실제로 쓰는 연산과 조합을
//! Vulkan 장치에서 실행해 CPU 결과와 대조합니다. logis-center/commerce 의 tests/it/vulkan.rs 와 같은 방식입니다.
//!
//! 포크된 candle 은 소프트웨어 Vulkan(llvmpipe/lavapipe)이나 작은 텐서에서는 기본적으로
//! "CPU 코드 + Vulkan 매핑 메모리" 경로를 탑니다. 그래서 모든 비교를 두 모드로 실행합니다.
//!   - native=false : 기본 경로
//!   - native=true  : GPU 셰이더 강제 + 크기 임계값 0 (작은 텐서도 셰이더로 실행)
//! 셰이더 설정은 프로세스 전역이므로 모든 테스트가 `MODE_LOCK` 을 잡고 실행합니다.
//!
//! GPU 가 없는 리눅스에서는 `CANDLE_VULKAN_ALLOW_CPU=1` (mesa lavapipe) 로 실행하세요. (test_linux.sh 가 설정)
#![cfg(feature = "vulkan")]

use candle_core::vulkan_backend::shaders;
use candle_core::{DType, Device, IndexOp, Module, Tensor, D};
use candle_nn::{VarBuilder, VarMap};
use endmill_model::ml;
use endmill_model::ml::modernbert::{ModernBert, ModernBertConfig};
use std::collections::HashMap;
use std::sync::Mutex;

type R = anyhow::Result<()>;

static MODE_LOCK: Mutex<()> = Mutex::new(());

const NO_DEVICE_HINT: &str = "no usable Vulkan device. The fork skips CPU-type devices by default; \
on a machine without a GPU run with CANDLE_VULKAN_ALLOW_CPU=1 (mesa llvmpipe/lavapipe)";

fn vk() -> Device {
    let d = ml::new_gpu_device(0).expect(NO_DEVICE_HINT);
    assert!(d.is_vulkan(), "expected a Vulkan device, got {d:?}");
    d
}

/// native 커널 off / on(임계값 0) 두 모드로 `f` 를 실행합니다.
fn with_modes(f: impl Fn(&Device) -> R) -> R {
    let _guard = MODE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dev = vk();
    for native in [false, true] {
        shaders::set_native_override(Some(native));
        shaders::set_native_thresholds(Some(0), Some(0));
        let res = f(&dev);
        shaders::set_native_override(None);
        shaders::set_native_thresholds(None, None);
        res.map_err(|e| e.context(format!("native kernels: {native}")))?;
    }
    Ok(())
}

fn to_vec_f64(t: &Tensor) -> anyhow::Result<Vec<f64>> {
    Ok(t.to_device(&Device::Cpu)?.to_dtype(DType::F64)?.flatten_all()?.to_vec1::<f64>()?)
}

/// 원소별 |got-want| <= tol * max(|got|,|want|,1)
fn assert_close(got: &Tensor, want: &Tensor, tol: f64, what: &str) -> R {
    assert_eq!(got.dims(), want.dims(), "{what}: shape mismatch");
    let g = to_vec_f64(got)?;
    let w = to_vec_f64(want)?;
    for (i, (x, y)) in g.iter().zip(w.iter()).enumerate() {
        if x == y || (x.is_nan() && y.is_nan()) {
            continue;
        }
        assert!(x.is_finite(), "{what}: non-finite value {x} at {i} (cpu {y})");
        let scale = x.abs().max(y.abs()).max(1.0);
        assert!((x - y).abs() <= tol * scale, "{what}: mismatch at {i}: vulkan {x} vs cpu {y}");
    }
    Ok(())
}

fn randn(shape: &[usize]) -> anyhow::Result<Tensor> {
    Ok(Tensor::randn(0f32, 1f32, shape, &Device::Cpu)?)
}

/// 같은 연산을 CPU 와 Vulkan 에서 실행해 비교합니다.
fn same_on_both(
    dev: &Device,
    inputs: &[&Tensor],
    tol: f64,
    what: &str,
    f: impl Fn(&[Tensor]) -> candle_core::Result<Tensor>,
) -> R {
    let cpu_in: Vec<Tensor> = inputs.iter().map(|t| (*t).clone()).collect();
    let vk_in: Vec<Tensor> = inputs.iter().map(|t| t.to_device(dev)).collect::<Result<_, _>>()?;
    let want = f(&cpu_in)?;
    let got = f(&vk_in)?;
    assert!(got.device().is_vulkan(), "{what}: result left the Vulkan device");
    assert_close(&got, &want, tol, what)
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. 장치 선택 · 라벨 · VRAM 조회 (src/ml/mod.rs)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn device_selection_label_and_mem_info() {
    let _guard = MODE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let d = vk();
    let label = ml::device_label(&d);
    eprintln!("Vulkan device: {label}");
    assert!(label.starts_with("Vulkan:0"), "unexpected label {label}");
    assert!(ml::gpu_name(&d).is_some());
    let (free, total) = ml::gpu_mem_info(&d).expect("gpu_mem_info");
    assert!(total > 0 && free <= total, "free={free} total={total}");
    assert!(ml::gpu_mem_info(&Device::Cpu).is_none());
    assert_eq!(ml::device_label(&Device::Cpu), "CPU");
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. 모델이 쓰는 연산 (attention · norm · 활성함수 · 패치 임베딩)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn attention_patterns_used_by_models() -> R {
    // (b, heads, n, hd) — siglip/laya/modernbert 의 q·kᵀ → softmax → ·v
    let q = randn(&[2, 4, 9, 16])?;
    let k = randn(&[2, 4, 9, 16])?;
    let v = randn(&[2, 4, 9, 16])?;
    // ModernBERT 슬라이딩 윈도우 마스크 (-inf 포함) + 패딩 마스크(-1e30)
    let n = 9usize;
    let mut local = vec![0f32; n * n];
    for i in 0..n {
        for j in 0..n {
            if (i as i64 - j as i64).unsigned_abs() > 2 {
                local[i * n + j] = f32::NEG_INFINITY;
            }
        }
    }
    let local = Tensor::from_vec(local, (1, 1, n, n), &Device::Cpu)?;
    let mut pad = vec![0f32; 2 * n];
    pad[n + 7] = -1e30;
    pad[n + 8] = -1e30;
    let pad = Tensor::from_vec(pad, (2, 1, 1, n), &Device::Cpu)?;
    with_modes(|dev| {
        same_on_both(dev, &[&q, &k, &v, &local, &pad], 1e-4, "masked attention", |t| {
            let att = (t[0].matmul(&t[1].transpose(D::Minus2, D::Minus1)?)? * 0.25)?;
            let att = att.broadcast_add(&t[3])?.broadcast_add(&t[4])?;
            let att = candle_nn::ops::softmax_last_dim(&att)?;
            att.matmul(&t[2])?.transpose(1, 2)?.reshape((2, n, 64))
        })?;
        same_on_both(dev, &[&q, &k], 1e-4, "k.t() attention scores", |t| {
            t[0].matmul(&t[1].t()?)? / 4.0
        })?;
        Ok(())
    })
}

#[test]
fn norm_and_activation_ops_used_by_models() -> R {
    let x = randn(&[2, 6, 96])?;
    let w = ((randn(&[96])? * 0.1)? + 1.0)?;
    let b = (randn(&[96])? * 0.1)?;
    with_modes(|dev| {
        same_on_both(dev, &[&x, &w, &b], 1e-4, "LayerNorm (bias)", |t| {
            candle_nn::LayerNorm::new(t[1].clone(), t[2].clone(), 1e-6).forward(&t[0])
        })?;
        same_on_both(dev, &[&x, &w], 1e-4, "LayerNorm (no bias, ModernBERT)", |t| {
            candle_nn::LayerNorm::new_no_bias(t[1].clone(), 1e-5).forward(&t[0])
        })?;
        same_on_both(dev, &[&x], 1e-4, "gelu_erf", |t| t[0].gelu_erf())?;
        same_on_both(dev, &[&x], 1e-4, "gelu(tanh) · relu", |t| t[0].gelu()? + t[0].relu()?)?;
        same_on_both(dev, &[&x], 1e-4, "GeGLU (chunk → gelu_erf * gate)", |t| {
            let parts = t[0].chunk(2, D::Minus1)?;
            parts[0].gelu_erf()? * &parts[1]
        })?;
        same_on_both(dev, &[&x], 1e-5, "softmax_last_dim", |t| candle_nn::ops::softmax_last_dim(&t[0]))?;
        same_on_both(dev, &[&x], 1e-4, "TTM 표준화 (mean/var → broadcast)", |t| {
            let mean = t[0].mean_keepdim(D::Minus1)?;
            let xc = t[0].broadcast_sub(&mean)?;
            let denom = (xc.sqr()?.mean_keepdim(D::Minus1)? + 1e-5)?.sqrt()?;
            xc.broadcast_div(&denom)
        })?;
        Ok(())
    })
}

#[test]
fn rope_and_indexing_ops_used_by_models() -> R {
    let q = randn(&[1, 4, 8, 16])?;
    let cos = randn(&[8, 8])?.cos()?;
    let sin = randn(&[8, 8])?.sin()?;
    let table = randn(&[32, 24])?;
    let idx = Tensor::new(&[3u32, 0, 31, 7, 7], &Device::Cpu)?;
    with_modes(|dev| {
        same_on_both(dev, &[&q, &cos, &sin], 1e-4, "rotary_emb::rope (ModernBERT)", |t| {
            candle_nn::rotary_emb::rope(&t[0].contiguous()?, &t[1], &t[2])
        })?;
        same_on_both(dev, &[&table, &idx], 1e-6, "index_select (TTM FFT 표 · 임베딩)", |t| {
            t[0].index_select(&t[1], 0)
        })?;
        same_on_both(dev, &[&table], 1e-6, "narrow · i() · permute", |t| {
            t[0].i((2..10, ..))?.narrow(1, 4, 8)?.reshape((2, 4, 8))?.permute((2, 0, 1))?.contiguous()
        })?;
        Ok(())
    })
}

#[test]
fn siglip_patch_embedding_and_position_interpolation() -> R {
    // SigLIP 패치 임베딩: conv2d(stride = patch) → flatten → transpose
    let pixel = randn(&[1, 3, 32, 32])?;
    let w = (randn(&[24, 3, 8, 8])? * 0.05)?;
    let pos = randn(&[1, 24, 4, 4])?;
    with_modes(|dev| {
        same_on_both(dev, &[&pixel, &w], 1e-4, "conv2d patch embed", |t| {
            t[0].conv2d(&t[1], 0, 8, 1, 1)?.flatten_from(2)?.transpose(1, 2)
        })?;
        same_on_both(dev, &[&pos], 1e-5, "interpolate2d (위치 임베딩 리사이즈)", |t| t[0].interpolate2d(6, 6))?;
        Ok(())
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. 작은 ModernBERT (laya 인코더) 전체 순전파 — 전역/슬라이딩 층 + 패딩 마스크
// ─────────────────────────────────────────────────────────────────────────────

fn tiny_modernbert_cfg() -> ModernBertConfig {
    ModernBertConfig::from_json(&serde_json::json!({
        "vocab_size": 64,
        "hidden_size": 32,
        "num_hidden_layers": 3,
        "num_attention_heads": 4,
        "intermediate_size": 48,
        "local_attention": 4,
        "global_attn_every_n_layers": 2,
        "norm_bias": false,
        "attention_bias": false,
        "mlp_bias": false
    }))
    .expect("tiny ModernBERT config")
}

#[test]
fn tiny_modernbert_matches_cpu_with_padding_and_local_window() -> R {
    let cfg = tiny_modernbert_cfg();
    let vm = VarMap::new();
    let cpu_model = ModernBert::load(&cfg, VarBuilder::from_varmap(&vm, DType::F32, &Device::Cpu))?;
    let weights: HashMap<String, Tensor> = vm
        .data()
        .lock()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_tensor().clone()))
        .collect();
    let ids = Tensor::new(&[[1u32, 5, 9, 2, 33, 7, 0, 0], [4, 8, 15, 16, 23, 42, 63, 3]], &Device::Cpu)?;
    let mask = Tensor::new(&[[1u32, 1, 1, 1, 1, 1, 0, 0], [1, 1, 1, 1, 1, 1, 1, 1]], &Device::Cpu)?;
    let want = cpu_model.forward(&ids, Some(&mask))?;
    with_modes(|dev| {
        let w: HashMap<String, Tensor> =
            weights.iter().map(|(k, v)| Ok((k.clone(), v.to_device(dev)?))).collect::<anyhow::Result<_>>()?;
        let model = ModernBert::load(&cfg, VarBuilder::from_tensors(w, DType::F32, dev))?;
        let got = model.forward(&ids.to_device(dev)?, Some(&mask.to_device(dev)?))?;
        assert!(got.device().is_vulkan());
        assert_close(&got, &want, 1e-3, "tiny ModernBERT hidden states")
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. 메모리 풀 정리 (flush_device / trim_idle_gpu_pool)
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn flush_device_releases_pooled_buffers() -> R {
    let _guard = MODE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dev = ml::device();
    if !dev.is_vulkan() {
        eprintln!("ENDMILL_DEVICE 가 Vulkan 이 아닌 장치를 골랐습니다 ({dev:?}) — 건너뜀");
        return Ok(());
    }
    {
        let a = Tensor::zeros((1024, 1024), DType::F32, &dev)?;
        let b = (a + 1.0)?;
        assert_eq!(b.sum_all()?.to_scalar::<f32>()?, (1024 * 1024) as f32);
    }
    ml::flush_device();
    let vd = dev.as_vulkan_device().expect("vulkan device");
    assert_eq!(vd.pooled_bytes(), 0, "pool not trimmed after flush_device");

    {
        let a = Tensor::zeros((4096, 4096), DType::F32, &dev)?;
        let _ = (a + 1.0)?;
    }
    ml::trim_idle_gpu_pool(&dev);
    assert!(vd.pooled_bytes() < (64 << 20), "trim_idle_gpu_pool left {} bytes", vd.pooled_bytes());
    Ok(())
}
