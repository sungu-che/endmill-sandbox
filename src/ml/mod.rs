pub mod laya;
pub mod modernbert;
pub mod siglip;
pub mod ttm;

use candle_core::{DType, Device};
use candle_nn::VarBuilder;
use std::path::{Path, PathBuf};

pub fn cpu() -> Device {
    Device::Cpu
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