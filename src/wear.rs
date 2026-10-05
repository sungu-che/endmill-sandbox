use crate::ml::siglip::SiglipModel;
use crate::ml::{cosine, l2_normalize};
use crate::toolimage::{self, EndProfile, SideProfile, ToolDims, ToolView};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

pub const TOOL_PROMPTS: &[(&str, &str, &str)] = &[
    ("new", "a close-up photo of a new carbide end mill with sharp intact cutting edges", "신품(정상 날)"),
    ("flank_wear", "a close-up photo of a worn end mill with a flank wear land on the cutting edge", "플랭크 마모"),
    ("chipping", "a close-up photo of an end mill with a chipped cutting edge", "치핑(결손)"),
    ("bue", "a close-up photo of an end mill with built-up edge, metal welded onto the cutting edge", "구성인선(용착)"),
    ("delamination", "a close-up photo of an end mill with peeling coating delamination", "코팅 박리"),
    ("thermal_crack", "a close-up photo of an end mill with thermal cracks on the cutting edge", "열균열"),
    ("corner_break", "a close-up photo of an end mill with a broken corner", "코너 파손"),
    ("heat_discoloration", "a close-up photo of an end mill with heat discoloration from overheating", "열변색"),
];

pub const MOLD_PROMPTS: &[(&str, &str, &str)] = &[
    ("mirror", "a close-up photo of a machined steel mold surface with a smooth mirror finish", "경면(양호)"),
    ("chatter", "a close-up photo of a machined metal surface with chatter marks", "채터 자국"),
    ("burn", "a close-up photo of a machined metal surface with burn marks and heat discoloration", "소착·열변색"),
    ("scallop", "a close-up photo of a machined metal surface with visible tool path scallops", "스캘럽 자국"),
    ("burr", "a close-up photo of a machined metal edge with burrs", "버(Burr)"),
    ("tearing", "a close-up photo of a machined metal surface with a torn rough texture", "뜯김·거친 면"),
];

#[derive(Debug, Clone)]
pub struct PromptBank {
    pub keys: Vec<String>,
    pub texts: Vec<String>,
    pub labels: Vec<String>,
    pub embeds: Vec<Vec<f32>>,
}

impl PromptBank {
    pub fn empty() -> Self {
        Self {
            keys: Vec::new(),
            texts: Vec::new(),
            labels: Vec::new(),
            embeds: Vec::new(),
        }
    }

    pub fn build(model: &SiglipModel, prompts: &[(&str, &str, &str)]) -> Result<Self, String> {
        let texts: Vec<String> = prompts.iter().map(|p| p.1.to_string()).collect();
        let mut embeds = model.embed_texts(&texts)?;
        for e in embeds.iter_mut() {
            l2_normalize(e);
        }
        Ok(Self {
            keys: prompts.iter().map(|p| p.0.to_string()).collect(),
            texts,
            labels: prompts.iter().map(|p| p.2.to_string()).collect(),
            embeds,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptScore {
    pub key: String,
    pub label: String,
    pub probability: f32,
    pub logit: f32,
}

pub fn zero_shot(model: &SiglipModel, bank: &PromptBank, image_vec: &[f32]) -> Vec<PromptScore> {
    bank.keys
        .iter()
        .enumerate()
        .map(|(i, k)| PromptScore {
            key: k.clone(),
            label: bank.labels[i].clone(),
            probability: model.probability(image_vec, &bank.embeds[i]),
            logit: model.logit(image_vec, &bank.embeds[i]),
        })
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageRecord {
    pub key: String,
    pub domain: String,
    pub view: ToolView,
    pub created_at: i64,
    pub grid: usize,
    pub global: Vec<f32>,
    #[serde(skip)]
    pub patches: Vec<Vec<f32>>,
    #[serde(skip)]
    pub dense: Vec<Vec<f32>>,
    pub attention: Vec<f32>,
    pub zone_of_patch: Vec<String>,
    pub side: Option<SideProfile>,
    pub end: Option<EndProfile>,
    pub zero_shot: Vec<PromptScore>,
    pub aligned_png_b64: String,
    pub notes: Vec<String>,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn analyze_tool_image(model: &SiglipModel, bank: &PromptBank, bytes: &[u8], view: ToolView, dims: &ToolDims, key: &str, px_hint: Option<f64>, tip_hint: Option<&str>) -> Result<ImageRecord, String> {
    let size = model.image_size();
    let prep = toolimage::prepare(bytes, view, dims, size, px_hint, tip_hint)?;
    let out = model.embed_image(prep.aligned.as_raw(), true)?;
    let mut global = out.pooled.clone();
    l2_normalize(&mut global);
    let norm_rows = |rows: Vec<Vec<f32>>| -> Vec<Vec<f32>> {
        rows.into_iter()
            .map(|mut r| {
                l2_normalize(&mut r);
                r
            })
            .collect()
    };
    let zone_of_patch = if prep.zone_of_patch.len() == out.grid * out.grid {
        prep.zone_of_patch.clone()
    } else {
        vec!["transition".to_string(); out.grid * out.grid]
    };
    Ok(ImageRecord {
        key: key.into(),
        domain: "tool.image".into(),
        view,
        created_at: now_ms(),
        grid: out.grid,
        zero_shot: zero_shot(model, bank, &global),
        global,
        patches: norm_rows(out.patches),
        dense: norm_rows(out.dense),
        attention: out.attention,
        zone_of_patch,
        side: prep.side,
        end: prep.end,
        aligned_png_b64: toolimage::png_base64(&prep.aligned),
        notes: prep.notes,
    })
}

pub fn analyze_mold_image(model: &SiglipModel, bank: &PromptBank, bytes: &[u8], key: &str) -> Result<ImageRecord, String> {
    let size = model.image_size();
    let sq = toolimage::center_square(bytes, size)?;
    let out = model.embed_image(sq.as_raw(), true)?;
    let mut global = out.pooled.clone();
    l2_normalize(&mut global);
    let grid = out.grid;
    let zone_of_patch: Vec<String> = (0..grid * grid)
        .map(|k| {
            let (i, j) = (k % grid, k / grid);
            let c = (grid as f64 - 1.0) / 2.0;
            let r = (((i as f64 - c).powi(2) + (j as f64 - c).powi(2)).sqrt()) / c.max(1.0);
            if r < 0.5 { "center" } else { "periphery" }.to_string()
        })
        .collect();
    let norm_rows = |rows: Vec<Vec<f32>>| -> Vec<Vec<f32>> {
        rows.into_iter()
            .map(|mut r| {
                l2_normalize(&mut r);
                r
            })
            .collect()
    };
    Ok(ImageRecord {
        key: key.into(),
        domain: "mold.image".into(),
        view: ToolView::Side,
        created_at: now_ms(),
        grid,
        zero_shot: zero_shot(model, bank, &global),
        global,
        patches: norm_rows(out.patches),
        dense: norm_rows(out.dense),
        attention: out.attention,
        zone_of_patch,
        side: None,
        end: None,
        aligned_png_b64: toolimage::png_base64(&sq),
        notes: Vec::new(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneResult {
    pub zone: String,
    pub label: String,
    pub patches: usize,
    pub mean_distance: f32,
    pub top_distance: f32,
    pub z: f32,
    pub embed_distance: f32,
    pub prompt_delta: Vec<(String, f32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeomWear {
    pub z_mm: Vec<f64>,
    pub radial_loss_um: Vec<f64>,
    pub max_loss_um: f64,
    pub mean_loss_tip_um: f64,
    pub flank_from_mm: f64,
    pub helix_shift_mm: f64,
    pub corner_r_ref_mm: f64,
    pub corner_r_cur_mm: f64,
    pub corner_wear_um: f64,
    pub edge_roughness_ref_um: f64,
    pub edge_roughness_cur_um: f64,
    pub per_flute_loss_um: Vec<f64>,
    pub scale_note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum WearState {
    Normal,
    Watch,
    Alert,
    Replace,
}

impl WearState {
    pub fn key(&self) -> &'static str {
        match self {
            WearState::Normal => "normal",
            WearState::Watch => "watch",
            WearState::Alert => "alert",
            WearState::Replace => "replace",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            WearState::Normal => "정상",
            WearState::Watch => "관찰",
            WearState::Alert => "경고",
            WearState::Replace => "교체",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WearComparison {
    pub domain: String,
    pub reference_key: String,
    pub current_key: String,
    pub grid: usize,
    pub global_distance: f32,
    pub background_mean: f32,
    pub background_sd: f32,
    pub patch_distance: Vec<f32>,
    pub patch_z: Vec<f32>,
    pub zones: Vec<ZoneResult>,
    pub global_prompt_delta: Vec<(String, String, f32, f32, f32)>,
    pub geometry: Option<GeomWear>,
    pub state: WearState,
    pub state_reasons: Vec<String>,
    pub wear_type: String,
    pub wear_type_label: String,
    pub type_scores: Vec<(String, f32)>,
    pub severity_index: f32,
    pub sds_z: Option<f32>,
    pub notes: Vec<String>,
}

pub fn zone_label(z: &str) -> &'static str {
    match z {
        "corner_left" => "좌측 코너",
        "corner_right" => "우측 코너",
        "end_edge" => "저면 날",
        "flank_left" => "좌측 플랭크(하단)",
        "flank_right" => "우측 플랭크(하단)",
        "flank_upper" => "플랭크(상단)",
        "flute_body" => "플루트 몸체",
        "shank" => "샹크",
        "center" => "중심부",
        "end_mid" => "저면 중간",
        "outer_ring" => "외곽 날끝",
        "periphery" => "주변부",
        "transition" => "경계",
        _ => "기타",
    }
}

fn edge_zone(z: &str) -> bool {
    matches!(z, "corner_left" | "corner_right" | "end_edge" | "flank_left" | "flank_right" | "outer_ring")
}

fn mean_sd(v: &[f32]) -> (f32, f32) {
    if v.is_empty() {
        return (0.0, 0.0);
    }
    let m = v.iter().sum::<f32>() / v.len() as f32;
    let s = (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32).sqrt();
    (m, s)
}

pub struct CompareLimits {
    pub radial_limit_um: f64,
    pub tool_diameter_mm: f64,
    pub loc_mm: f64,
    pub flutes: u32,
    pub helix_deg: f64,
    pub same_setup: bool,
}

fn resample(z: &[f64], v: &[f64], at: &[f64]) -> Vec<f64> {
    at.iter()
        .map(|t| {
            if z.is_empty() {
                return f64::NAN;
            }
            let mut best = f64::NAN;
            for i in 0..z.len().saturating_sub(1) {
                let (a, b) = (z[i], z[i + 1]);
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                if *t >= lo && *t <= hi {
                    let w = if (b - a).abs() > 1e-12 { (t - a) / (b - a) } else { 0.0 };
                    best = v[i] * (1.0 - w) + v[i + 1] * w;
                    break;
                }
            }
            best
        })
        .collect()
}

fn side_geometry(r: &SideProfile, c: &SideProfile, lim: &CompareLimits) -> GeomWear {
    let lead = std::f64::consts::PI * lim.tool_diameter_mm / lim.helix_deg.to_radians().tan().max(0.05);
    let pitch = lead / lim.flutes.max(1) as f64;
    let zmax = lim.loc_mm.min(r.z_mm.iter().cloned().fold(0.0, f64::max)).min(c.z_mm.iter().cloned().fold(0.0, f64::max));
    let samples = 160usize;
    let grid: Vec<f64> = (0..samples).map(|i| zmax * (i as f64 + 0.5) / samples as f64).collect();
    let rw = resample(&r.z_mm, &r.width_mm, &grid);
    let mut best = (0.0, f64::INFINITY);
    let align_lo = 0.55 * zmax;
    let steps = 41;
    for k in 0..steps {
        let s = -pitch / 2.0 + pitch * k as f64 / (steps - 1) as f64;
        let shifted: Vec<f64> = grid.iter().map(|z| z + s).collect();
        let cw = resample(&c.z_mm, &c.width_mm, &shifted);
        let mut err = 0.0;
        let mut n = 0.0;
        for i in 0..samples {
            if grid[i] < align_lo || rw[i].is_nan() || cw[i].is_nan() {
                continue;
            }
            err += (rw[i] - cw[i]).powi(2);
            n += 1.0;
        }
        if n > 5.0 && err / n < best.1 {
            best = (s, err / n);
        }
    }
    let shift = best.0;
    let shifted: Vec<f64> = grid.iter().map(|z| z + shift).collect();
    let rl = resample(&r.z_mm, &r.left_mm, &grid);
    let rr = resample(&r.z_mm, &r.right_mm, &grid);
    let cl = resample(&c.z_mm, &c.left_mm, &shifted);
    let cr = resample(&c.z_mm, &c.right_mm, &shifted);
    let loss: Vec<f64> = (0..samples)
        .map(|i| {
            let l = if rl[i].is_nan() || cl[i].is_nan() { 0.0 } else { (rl[i] - cl[i]) * 1000.0 };
            let rgt = if rr[i].is_nan() || cr[i].is_nan() { 0.0 } else { (rr[i] - cr[i]) * 1000.0 };
            l.max(rgt).max(-50.0)
        })
        .collect();
    let corner_ref = (r.corner_r_left_mm + r.corner_r_right_mm) / 2.0;
    let corner_cur = (c.corner_r_left_mm + c.corner_r_right_mm) / 2.0;
    let flank_lo = (0.25 * lim.tool_diameter_mm).max(1.5 * corner_ref.max(corner_cur));
    let flank: Vec<f64> = grid
        .iter()
        .zip(loss.iter())
        .filter(|(z, _)| **z >= flank_lo)
        .map(|(_, v)| *v)
        .collect();
    let tip: Vec<f64> = grid
        .iter()
        .zip(loss.iter())
        .filter(|(z, _)| **z >= flank_lo && **z <= flank_lo + 0.5 * lim.tool_diameter_mm)
        .map(|(_, v)| *v)
        .collect();
    GeomWear {
        max_loss_um: flank.iter().cloned().fold(0.0, f64::max),
        mean_loss_tip_um: if tip.is_empty() { 0.0 } else { tip.iter().sum::<f64>() / tip.len() as f64 },
        flank_from_mm: flank_lo,
        z_mm: grid,
        radial_loss_um: loss,
        helix_shift_mm: shift,
        corner_r_ref_mm: corner_ref,
        corner_r_cur_mm: corner_cur,
        corner_wear_um: ((corner_cur - corner_ref) * 1000.0).max(0.0),
        edge_roughness_ref_um: r.edge_roughness_um,
        edge_roughness_cur_um: c.edge_roughness_um,
        per_flute_loss_um: Vec::new(),
        scale_note: format!("기준 {} / 현재 {}", r.calibration, c.calibration),
    }
}

fn end_geometry(r: &EndProfile, c: &EndProfile, lim: &CompareLimits) -> GeomWear {
    let n = r.radius_mm.len().min(c.radius_mm.len());
    let scale = if lim.same_setup { r.px_per_mm / c.px_per_mm.max(1e-9) } else { 1.0 };
    let cur: Vec<f64> = c.radius_mm.iter().map(|v| v * scale).collect();
    let mut best = (0usize, f64::INFINITY);
    for s in 0..n {
        let err: f64 = (0..n).map(|i| (r.radius_mm[i] - cur[(i + s) % n]).powi(2)).sum();
        if err < best.1 {
            best = (s, err);
        }
    }
    let loss: Vec<f64> = (0..n).map(|i| (r.radius_mm[i] - cur[(i + best.0) % n]) * 1000.0).collect();
    let mut peaks: Vec<(usize, f64)> = (0..n)
        .filter(|i| {
            let a = r.radius_mm[(i + n - 3) % n];
            let b = r.radius_mm[(i + 3) % n];
            r.radius_mm[*i] >= a && r.radius_mm[*i] >= b
        })
        .map(|i| (i, r.radius_mm[i]))
        .collect();
    peaks.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut chosen: Vec<usize> = Vec::new();
    for (i, _) in peaks.iter() {
        if chosen.iter().all(|c| {
            let d = (*c as i64 - *i as i64).unsigned_abs() as usize;
            d.min(n - d) > n / (2 * lim.flutes.max(1) as usize)
        }) {
            chosen.push(*i);
        }
        if chosen.len() >= lim.flutes as usize {
            break;
        }
    }
    chosen.sort();
    let per_flute: Vec<f64> = chosen.iter().map(|i| loss[*i]).collect();
    GeomWear {
        z_mm: (0..n).map(|i| i as f64).collect(),
        flank_from_mm: 0.0,
        max_loss_um: per_flute.iter().cloned().fold(0.0, f64::max),
        mean_loss_tip_um: if per_flute.is_empty() { 0.0 } else { per_flute.iter().sum::<f64>() / per_flute.len() as f64 },
        radial_loss_um: loss,
        helix_shift_mm: best.0 as f64,
        corner_r_ref_mm: 0.0,
        corner_r_cur_mm: 0.0,
        corner_wear_um: 0.0,
        edge_roughness_ref_um: 0.0,
        edge_roughness_cur_um: 0.0,
        per_flute_loss_um: per_flute,
        scale_note: if lim.same_setup {
            "동일 촬영 조건: 기준 배율 사용".into()
        } else {
            format!("기준 {} / 현재 {} (배율 독립 보정)", r.calibration, c.calibration)
        },
    }
}

pub fn compare(model: &SiglipModel, bank: &PromptBank, reference: &ImageRecord, current: &ImageRecord, lim: &CompareLimits, sds_baseline: Option<(f32, f32, u64)>) -> Result<WearComparison, String> {
    if reference.patches.len() != current.patches.len() || reference.patches.is_empty() {
        return Err("기준/현재 이미지의 패치 수가 다르거나 패치 벡터가 없습니다 (기준 이미지를 다시 등록하세요)".into());
    }
    let n = reference.patches.len();
    let surface = reference.domain == "mold.image";
    let dist: Vec<f32> = (0..n).map(|i| 1.0 - cosine(&reference.patches[i], &current.patches[i])).collect();
    let bg: Vec<f32> = if surface {
        dist.clone()
    } else {
        (0..n)
            .filter(|i| reference.zone_of_patch[*i].as_str() == "background")
            .map(|i| dist[i])
            .collect()
    };
    let (bg_m, bg_s) = if bg.len() >= 8 { mean_sd(&bg) } else { mean_sd(&dist) };
    let bg_s = bg_s.max(0.02);
    let patch_z: Vec<f32> = dist.iter().map(|d| (d - bg_m) / bg_s).collect();
    let mut zones_map: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, z) in reference.zone_of_patch.iter().enumerate() {
        if z == "background" || z == "outside" {
            continue;
        }
        zones_map.entry(z.clone()).or_default().push(i);
    }
    let mut zones = Vec::new();
    for (zone, idx) in zones_map.iter() {
        let mut ds: Vec<f32> = idx.iter().map(|i| dist[*i]).collect();
        let mean = ds.iter().sum::<f32>() / ds.len().max(1) as f32;
        ds.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        let k = (ds.len() / 4).max(1);
        let top = ds[..k].iter().sum::<f32>() / k as f32;
        let avg = |rec: &ImageRecord| -> Vec<f32> {
            let d = rec.dense.first().map(|r| r.len()).unwrap_or(0);
            let mut v = vec![0f32; d];
            for i in idx.iter() {
                if let Some(row) = rec.dense.get(*i) {
                    for (a, b) in v.iter_mut().zip(row.iter()) {
                        *a += *b;
                    }
                }
            }
            l2_normalize(&mut v);
            v
        };
        let rz = avg(reference);
        let cz = avg(current);
        let prompt_delta: Vec<(String, f32)> = bank
            .keys
            .iter()
            .enumerate()
            .map(|(pi, key)| {
                let pr = model.probability(&rz, &bank.embeds[pi]);
                let pc = model.probability(&cz, &bank.embeds[pi]);
                (key.clone(), pc - pr)
            })
            .collect();
        zones.push(ZoneResult {
            zone: zone.clone(),
            label: zone_label(zone).into(),
            patches: idx.len(),
            mean_distance: mean,
            top_distance: top,
            z: (top - bg_m) / bg_s,
            embed_distance: 1.0 - cosine(&rz, &cz),
            prompt_delta,
        });
    }
    let global_prompt_delta: Vec<(String, String, f32, f32, f32)> = reference
        .zero_shot
        .iter()
        .zip(current.zero_shot.iter())
        .map(|(a, b)| (a.key.clone(), a.label.clone(), a.probability, b.probability, b.probability - a.probability))
        .collect();
    let geometry = match (&reference.side, &current.side, &reference.end, &current.end) {
        (Some(r), Some(c), _, _) => Some(side_geometry(r, c, lim)),
        (_, _, Some(r), Some(c)) => Some(end_geometry(r, c, lim)),
        _ => None,
    };
    let mut state = WearState::Normal;
    let mut reasons = Vec::new();
    let mut index = 0f64;
    if let Some(g) = &geometry {
        let ratio = g.max_loss_um / lim.radial_limit_um.max(1.0);
        index = index.max(ratio);
        let gs = if ratio >= 1.0 {
            WearState::Replace
        } else if ratio >= 0.8 {
            WearState::Alert
        } else if ratio >= 0.5 {
            WearState::Watch
        } else {
            WearState::Normal
        };
        if gs > WearState::Normal {
            reasons.push(format!(
                "플랭크 반경 손실 최대 {:.1} µm (한계 {:.1} µm 의 {:.0}%)",
                g.max_loss_um,
                lim.radial_limit_um,
                ratio * 100.0
            ));
        }
        let cr = g.corner_wear_um / lim.radial_limit_um.max(1.0);
        index = index.max(cr / 4.0);
        let cs = if cr >= 4.0 {
            WearState::Replace
        } else if cr >= 2.0 {
            WearState::Alert
        } else if cr >= 0.5 {
            WearState::Watch
        } else {
            WearState::Normal
        };
        if cs > WearState::Normal {
            reasons.push(format!(
                "코너 R {:.3} → {:.3} mm (증가 {:.1} µm, 한계 대비 {:.1}배)",
                g.corner_r_ref_mm,
                g.corner_r_cur_mm,
                g.corner_wear_um,
                cr
            ));
        }
        state = state.max(gs).max(cs);
    }
    let is_edge = |z: &str| surface || edge_zone(z);
    let edge_max = zones
        .iter()
        .filter(|z| is_edge(&z.zone))
        .map(|z| z.z)
        .fold(f32::NEG_INFINITY, f32::max);
    if edge_max.is_finite() {
        index = index.max(edge_max as f64 / 6.0);
        let is = if edge_max >= 4.5 {
            WearState::Alert
        } else if edge_max >= 3.0 {
            WearState::Watch
        } else {
            WearState::Normal
        };
        if is > WearState::Normal {
            reasons.push(format!("날 부위 패치 벡터 이탈 z={:.1} (배경 대비)", edge_max));
        }
        state = state.max(is);
    }
    let chip_delta = zones
        .iter()
        .filter(|z| is_edge(&z.zone))
        .flat_map(|z| z.prompt_delta.iter().filter(|(k, _)| k == "chipping" || k == "corner_break").map(|(_, d)| *d))
        .fold(0f32, f32::max);
    let rough_up = geometry
        .as_ref()
        .map(|g| g.edge_roughness_cur_um > 2.0 * g.edge_roughness_ref_um.max(1.0))
        .unwrap_or(false);
    if (chip_delta >= 0.15 && edge_max >= 2.5) || rough_up {
        reasons.push("치핑/결손 의심 (제로샷 증가 또는 날 윤곽 거칠기 2배 이상)".into());
        state = state.max(WearState::Alert);
    }
    let mut sds_z = None;
    let edge_top = zones.iter().filter(|z| is_edge(&z.zone)).map(|z| z.top_distance).fold(0f32, f32::max);
    if let Some((mu, sd, cnt)) = sds_baseline {
        if cnt >= 5 && sd > 1e-6 {
            let z = (edge_top - mu) / sd;
            sds_z = Some(z);
            if z >= 3.0 {
                reasons.push(format!("동일 스코프 이력 대비 날 부위 거리 드리프트 z={:.1}", z));
                state = state.max(WearState::Watch);
            }
        }
    }
    let mut type_scores: Vec<(String, f32)> = bank
        .keys
        .iter()
        .filter(|k| k.as_str() != "new" && k.as_str() != "mirror")
        .map(|k| {
            let g = global_prompt_delta.iter().find(|(kk, ..)| kk == k).map(|t| t.4).unwrap_or(0.0);
            let zmax = zones
                .iter()
                .filter(|z| is_edge(&z.zone))
                .flat_map(|z| z.prompt_delta.iter().filter(|(kk, _)| kk == k).map(|(_, d)| *d))
                .fold(f32::NEG_INFINITY, f32::max);
            (k.clone(), g.max(zmax))
        })
        .collect();
    type_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let (wear_type, wear_type_label) = match type_scores.first() {
        Some((k, d)) if *d > 0.02 => (
            k.clone(),
            bank.keys
                .iter()
                .position(|kk| kk == k)
                .map(|i| bank.labels[i].clone())
                .unwrap_or_default(),
        ),
        _ => ("none".into(), "뚜렷한 유형 변화 없음".into()),
    };
    let mut notes = reference.notes.clone();
    notes.extend(current.notes.iter().cloned());
    if bg.len() < 8 {
        notes.push("배경 패치가 부족해 전체 패치 분포로 z 를 표준화했습니다".into());
    }
    Ok(WearComparison {
        domain: reference.domain.clone(),
        reference_key: reference.key.clone(),
        current_key: current.key.clone(),
        grid: reference.grid,
        global_distance: 1.0 - cosine(&reference.global, &current.global),
        background_mean: bg_m,
        background_sd: bg_s,
        patch_distance: dist,
        patch_z,
        zones,
        global_prompt_delta,
        geometry,
        state,
        state_reasons: reasons,
        wear_type,
        wear_type_label,
        type_scores,
        severity_index: index as f32,
        sds_z,
        notes,
    })
}

pub fn save_record(dir: &Path, rec: &ImageRecord) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let base = sanitize(&format!("{}_{}_{}", rec.domain, rec.key, rec.view.key()));
    let meta = serde_json::to_string(rec).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("{}.json", base)), meta).map_err(|e| e.to_string())?;
    let mut f = std::fs::File::create(dir.join(format!("{}.bin", base))).map_err(|e| e.to_string())?;
    let rows = rec.patches.len() as u32;
    let dim = rec.patches.first().map(|r| r.len()).unwrap_or(0) as u32;
    let drows = rec.dense.len() as u32;
    f.write_all(&rows.to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(&dim.to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(&drows.to_le_bytes()).map_err(|e| e.to_string())?;
    let mut buf: Vec<u8> = Vec::with_capacity(((rows + drows) * dim) as usize * 4);
    for r in rec.patches.iter().chain(rec.dense.iter()) {
        for v in r.iter() {
            buf.extend_from_slice(&v.to_le_bytes());
        }
    }
    f.write_all(&buf).map_err(|e| e.to_string())
}

pub fn load_record(dir: &Path, domain: &str, key: &str, view: ToolView) -> Result<ImageRecord, String> {
    let base = sanitize(&format!("{}_{}_{}", domain, key, view.key()));
    let txt = std::fs::read_to_string(dir.join(format!("{}.json", base))).map_err(|e| e.to_string())?;
    let mut rec: ImageRecord = serde_json::from_str(&txt).map_err(|e| e.to_string())?;
    let mut f = std::fs::File::open(dir.join(format!("{}.bin", base))).map_err(|e| e.to_string())?;
    let mut hdr = [0u8; 12];
    f.read_exact(&mut hdr).map_err(|e| e.to_string())?;
    let rows = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
    let dim = u32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
    let drows = u32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;
    let mut body = Vec::new();
    f.read_to_end(&mut body).map_err(|e| e.to_string())?;
    if body.len() < (rows + drows) * dim * 4 {
        return Err("벡터 파일이 손상되었습니다".into());
    }
    let mut it = body.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
    rec.patches = (0..rows).map(|_| (0..dim).filter_map(|_| it.next()).collect()).collect();
    rec.dense = (0..drows).map(|_| (0..dim).filter_map(|_| it.next()).collect()).collect();
    Ok(rec)
}

pub fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' })
        .collect()
}

pub fn tool_dims_from_setting(s: &crate::profile::EndMillMockupSetting) -> ToolDims {
    ToolDims {
        diameter_mm: s.diameter_mm,
        shank_mm: s.shank_diameter_mm,
        loc_mm: s.loc_mm,
        corner_r_mm: s.nose.corner_radius(s.diameter_mm),
    }
}