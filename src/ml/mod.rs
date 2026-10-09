pub mod laya;
pub mod modernbert;
pub mod siglip;
pub mod ttm;

use candle_core::{DType, Device};
use candle_nn::VarBuilder;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub fn cpu() -> Device {
    Device::Cpu
}

static DEVICE: OnceLock<Device> = OnceLock::new();

pub fn device() -> Device {
    DEVICE
        .get_or_init(|| {
            let forced_cpu = std::env::var("ENDMILL_DEVICE")
                .map(|v| v.trim().eq_ignore_ascii_case("cpu"))
                .unwrap_or(false);
            if forced_cpu {
                return Device::Cpu;
            }
            new_gpu_device(0).unwrap_or(Device::Cpu)
        })
        .clone()
}

/// 빌드된 백엔드 feature 순서(cuda → rocm → vulkan)대로 GPU 장치를 시도합니다.
/// `ENDMILL_DEVICE=cuda|rocm|vulkan` 으로 특정 백엔드를 강제할 수 있습니다.
pub fn new_gpu_device(id: usize) -> Option<Device> {
    let want = std::env::var("ENDMILL_DEVICE")
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let allow = |name: &str| want.is_empty() || want == "gpu" || want == name;
    let _ = (&allow, id);
    #[cfg(feature = "cuda")]
    if allow("cuda") {
        if let Ok(d) = Device::new_cuda(id) {
            return Some(d);
        }
    }
    #[cfg(feature = "rocm")]
    if allow("rocm") {
        if let Ok(d) = Device::new_rocm(id) {
            return Some(d);
        }
    }
    #[cfg(feature = "vulkan")]
    if allow("vulkan") {
        if let Ok(d) = Device::new_vulkan(id) {
            return Some(d);
        }
    }
    None
}

pub fn device_label(d: &Device) -> String {
    let named = |kind: &str| match gpu_name(d) {
        Some(n) if !n.is_empty() => format!("{kind}:0 ({n})"),
        _ => format!("{kind}:0"),
    };
    if d.is_cuda() {
        "CUDA:0".into()
    } else if d.is_rocm() {
        named("ROCm")
    } else if d.is_vulkan() {
        named("Vulkan")
    } else if d.is_metal() {
        "Metal".into()
    } else {
        "CPU".into()
    }
}

/// CUDA·ROCm 처럼 장치 큐를 동기화해야 할당자가 버퍼를 실제로 돌려주는 백엔드인지.
pub trait GpuDeviceExt {
    fn is_cuda_or_rocm(&self) -> bool;
}

impl GpuDeviceExt for Device {
    fn is_cuda_or_rocm(&self) -> bool {
        self.is_cuda() || self.is_rocm()
    }
}

/// ROCm / Vulkan 장치 이름 (CUDA·CPU 는 None).
pub fn gpu_name(d: &Device) -> Option<String> {
    if let Ok(v) = d.as_vulkan_device() {
        return Some(format!("{}, {}", v.name(), v.device_type()));
    }
    if let Ok(r) = d.as_rocm_device() {
        return r.name().ok();
    }
    None
}

/// 장치의 (free, total) VRAM 바이트. NVML 없이 ROCm(hipMemGetInfo) / Vulkan(heap budget) 에서 조회합니다.
pub fn gpu_mem_info(d: &Device) -> Option<(u64, u64)> {
    if let Ok(r) = d.as_rocm_device() {
        return r.mem_info().ok().map(|(f, t)| (f as u64, t as u64));
    }
    if let Ok(v) = d.as_vulkan_device() {
        return v.mem_info().ok().map(|(f, t)| (f as u64, t as u64));
    }
    None
}

/// 추론 한 번이 끝난 뒤 재사용 풀에 남은 버퍼를 돌려줍니다 (logis-center 의 trim_idle_gpu_pool 과 동일한 정책).
/// Vulkan 은 풀이 64MB 이상일 때만, ROCm 은 항상 stream-ordered 풀을 trim 합니다.
pub fn trim_idle_gpu_pool(d: &Device) {
    if let Ok(v) = d.as_vulkan_device() {
        if v.pooled_bytes() >= (64 << 20) {
            let _ = v.trim_memory_pool();
        }
    } else if let Ok(r) = d.as_rocm_device() {
        let _ = r.trim_memory_pool();
    }
}

/// 이미 초기화된 공용 장치가 있을 때만 풀을 정리합니다 (장치를 새로 만들지 않음).
pub fn trim_active_pool() {
    if let Some(d) = DEVICE.get() {
        trim_idle_gpu_pool(d);
    }
}

pub fn ensure_safetensors_only(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Err(format!("모델 디렉터리가 없습니다: {}", dir.display()));
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {}", dir.display(), e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    let has_st = entries
        .iter()
        .any(|p| p.extension().map(|x| x == "safetensors").unwrap_or(false));
    let has_onnx = entries
        .iter()
        .any(|p| p.extension().map(|x| x == "onnx").unwrap_or(false));
    if !has_st && has_onnx {
        return Err(format!(
            "ONNX 가중치만 있습니다. 이 프로젝트는 safetensors/gguf 가중치만 사용합니다: {}",
            dir.display()
        ));
    }
    if !has_st {
        return Err(format!("safetensors 가중치를 찾을 수 없습니다: {}", dir.display()));
    }
    Ok(())
}

pub fn safetensor_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    ensure_safetensors_only(dir)?;
    let single = dir.join("model.safetensors");
    if single.exists() {
        return Ok(vec![single]);
    }
    let index = dir.join("model.safetensors.index.json");
    if index.exists() {
        let txt = std::fs::read_to_string(&index).map_err(|e| format!("{}: {}", index.display(), e))?;
        let v: serde_json::Value = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
        let mut files: Vec<String> = v
            .get("weight_map")
            .and_then(|m| m.as_object())
            .map(|m| m.values().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        files.sort();
        files.dedup();
        if !files.is_empty() {
            return Ok(files.into_iter().map(|f| dir.join(f)).collect());
        }
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {}", dir.display(), e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "safetensors").unwrap_or(false))
        .collect();
    found.sort();
    if found.is_empty() {
        Err(format!("safetensors 가중치를 찾을 수 없습니다: {}", dir.display()))
    } else {
        Ok(found)
    }
}

pub fn load_varbuilder(dir: &Path, device: &Device) -> Result<VarBuilder<'static>, String> {
    let files = safetensor_files(dir)?;
    unsafe { VarBuilder::from_mmaped_safetensors(&files, DType::F32, device) }.map_err(|e| e.to_string())
}

pub fn read_json(path: &Path) -> Result<serde_json::Value, String> {
    let txt = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    serde_json::from_str(&txt).map_err(|e| format!("{}: {}", path.display(), e))
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for i in 0..n {
        dot += a[i] as f64 * b[i] as f64;
        na += a[i] as f64 * a[i] as f64;
        nb += b[i] as f64 * b[i] as f64;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    (dot / (na.sqrt() * nb.sqrt())) as f32
}

pub fn l2_normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| (*x as f64) * (*x as f64)).sum::<f64>().sqrt();
    if n > 0.0 {
        for x in v.iter_mut() {
            *x = (*x as f64 / n) as f32;
        }
    }
}

pub fn flush_device() {
    // 아직 장치를 만든 적이 없으면 비울 것도 없습니다 (flush 가 GPU 초기화를 유발하지 않도록).
    let Some(dev) = DEVICE.get().cloned() else {
        return;
    };
    if dev.is_cpu() {
        return;
    }
    // 큐를 먼저 비워야 방금 drop 한 가중치 버퍼가 할당자에 반환됩니다.
    if dev.is_cuda_or_rocm() {
        let _ = dev.synchronize();
    }
    if let Ok(r) = dev.as_rocm_device() {
        if r.release_cached_resources().is_err() {
            let _ = r.trim_memory_pool();
        }
    }
    if let Ok(v) = dev.as_vulkan_device() {
        let _ = v.trim_memory_pool();
    }
    #[cfg(feature = "cuda")]
    {
        if dev.is_cuda() {
            use candle_core::{DType, Tensor};
            if let Ok(t) = Tensor::zeros((1,), DType::F32, &dev) {
                let _ = t.to_device(&dev);
            }
        }
    }
}

pub fn purge_device_memory() {
    flush_device();
}