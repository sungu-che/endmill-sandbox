use crate::cutting::{CoolantType, WorkpieceMaterial};
use crate::decision::{self, Action, DecisionInput, DecisionReport};
use crate::gcode::{GCodeGenerator, GCodeMetadata, ToolPathPattern, ToolPathSegment};
use crate::ingest::{self, DocKind, FieldValue, Issue, NormalizedDocument};
use crate::loadsim::{self, HeightmapView, LoadSimReport};
use crate::ml::laya::LayaAdvisor;
use crate::ml::siglip::SiglipModel;
use crate::ml::ttm::TtmForecaster;
use crate::mold::{self, DeviationFit, MoldComparison, MoldDescriptor, MoldPrediction, MoldVector, WallFit, WallPoint};
use crate::physics::{self, CutAnalysis, CutContext, Purpose, Recommendation, ThermalBody};
use crate::profile::{CoolantConfig, CoolantMethod, MachiningProfile};
use crate::sds::{Scope, SdsStore, Track};
use crate::timeseries::{self, ForecastResult, Series};
use crate::toolimage::{EndProfile, SideProfile, ToolView};
use crate::wear::{self, CompareLimits, GeomWear, ImageRecord, PromptBank, PromptScore, WearComparison, WearState};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn fingerprint(kind: &str, bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in kind.as_bytes().iter().chain(bytes.iter()) {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", h)
}

pub fn purpose_from_key(k: &str) -> Purpose {
    match k.trim().to_lowercase().as_str() {
        "finishing" | "finish" | "정삭" => Purpose::Finishing,
        _ => Purpose::Roughing,
    }
}

pub fn purpose_key(p: Purpose) -> &'static str {
    match p {
        Purpose::Roughing => "roughing",
        Purpose::Finishing => "finishing",
    }
}

pub fn default_purpose(p: &MachiningProfile) -> Purpose {
    if let Some(k) = p.purpose.as_deref().filter(|k| !k.trim().is_empty()) {
        return purpose_from_key(k);
    }
    let d = p.endmill_setting.diameter_mm;
    if p.workpiece_setup.effective_tolerance_mm() <= 0.02 && p.conditions.radial_doc_mm <= 0.15 * d {
        Purpose::Finishing
    } else {
        Purpose::Roughing
    }
}

pub fn material_scope_key(m: &WorkpieceMaterial) -> String {
    match m {
        WorkpieceMaterial::AlloySteel { hardness_hrc } => format!("alloy_steel_h{}", (*hardness_hrc / 5) * 5),
        other => other.family_key().to_string(),
    }
}

pub fn process_scope(p: &MachiningProfile) -> Scope {
    Scope::new(
        Track::Process,
        &material_scope_key(&p.workpiece_setup.effective_material()),
        &p.endmill_setting.signature(),
    )
}

pub fn decision_scope(p: &MachiningProfile) -> Scope {
    Scope::new(
        Track::Decision,
        &p.endmill_setting.signature(),
        &material_scope_key(&p.workpiece_setup.effective_material()),
    )
}

pub fn coolant_type_of(m: &CoolantMethod) -> CoolantType {
    match m {
        CoolantMethod::AirBlast => CoolantType::AirBlast,
        CoolantMethod::Flood => CoolantType::Flood,
        CoolantMethod::Mist => CoolantType::Mist,
        CoolantMethod::ThroughTool => CoolantType::ThroughTool,
        CoolantMethod::Dry => CoolantType::Dry,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationApplied {
    pub axis: String,
    pub factor: f64,
    pub n: u64,
    pub drift_z: f64,
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibEvent {
    pub axis: String,
    pub measured: f64,
    pub predicted: f64,
    pub source: String,
}

pub fn calibrated_context(p: &MachiningProfile, sds: Option<&SdsStore>) -> (CutContext, Vec<CalibrationApplied>) {
    let mut ctx = CutContext::from_profile(p);
    let mut applied = Vec::new();
    if let Some(store) = sds {
        let scope = process_scope(p);
        for axis in ["power", "wear", "deflection", "temperature"] {
            if let Some((factor, n, drift, key)) = store.calibration_factor(&scope, axis) {
                match axis {
                    "power" => ctx.calib.power = factor,
                    "wear" => ctx.calib.wear = factor,
                    "deflection" => ctx.calib.deflection = factor,
                    _ => ctx.calib.temperature = factor,
                }
                applied.push(CalibrationApplied {
                    axis: axis.into(),
                    factor,
                    n,
                    drift_z: drift,
                    scope: key,
                });
            }
        }
    }
    (ctx, applied)
}

pub fn shrink_heightmap(h: &HeightmapView, max_side: usize) -> HeightmapView {
    let side = h.nx.max(h.ny);
    if side <= max_side || max_side == 0 {
        return h.clone();
    }
    let f = (side + max_side - 1) / max_side;
    let nx = (h.nx + f - 1) / f;
    let ny = (h.ny + f - 1) / f;
    let mut z = vec![f32::NAN; nx * ny];
    for j in 0..ny {
        for i in 0..nx {
            let mut acc = 0.0f64;
            let mut n = 0.0f64;
            for jj in j * f..((j + 1) * f).min(h.ny) {
                for ii in i * f..((i + 1) * f).min(h.nx) {
                    let v = h.z[jj * h.nx + ii];
                    if v.is_finite() {
                        acc += v as f64;
                        n += 1.0;
                    }
                }
            }
            if n > 0.0 {
                z[j * nx + i] = (acc / n) as f32;
            }
        }
    }
    HeightmapView {
        x0: h.x0,
        y0: h.y0,
        cell_mm: h.cell_mm * f as f64,
        nx,
        ny,
        z,
        bottom: h.bottom,
    }
}

pub fn ingest_model_needs(name: &str, opt: &IngestOptions) -> Vec<String> {
    let lower = name.to_lowercase();
    let is_image = [".png", ".jpg", ".jpeg", ".bmp", ".tif", ".tiff", ".webp"].iter().any(|e| lower.ends_with(e));
    let geometry_role = matches!(opt.role.as_deref(), Some("nominal") | Some("measured"));
    let hint = opt.hint.as_deref().and_then(DocKind::from_key);
    let kind = match hint {
        Some(k) => Some(k),
        None if is_image && geometry_role => Some(DocKind::MoldDepth),
        None if is_image => Some(DocKind::ToolImage),
        None => None,
    };
    match kind {
        Some(DocKind::ToolImage) | Some(DocKind::MoldImage) => vec!["siglip".into()],
        _ => Vec::new(),
    }
}

pub fn trim_sim(r: &LoadSimReport) -> LoadSimReport {
    let mut out = r.clone();
    let stride = ((out.samples.len() + 999) / 1000).max(1);
    let last = out.samples.len().saturating_sub(1);
    out.samples = out
        .samples
        .iter()
        .enumerate()
        .filter(|(i, _)| i % stride == 0 || *i == last)
        .map(|(_, s)| s.clone())
        .collect();
    let wstride = ((out.wall_errors.len() + 199) / 200).max(1);
    out.wall_errors = out.wall_errors.iter().step_by(wstride).cloned().collect();
    out.heightmap = shrink_heightmap(&out.heightmap, 160);
    out.floor_error = shrink_heightmap(&out.floor_error, 96);
    out.first_cut = None;
    out.last_cut = None;
    out
}

pub const SIGLIP_KEYWORD: &str = "siglip";
pub const TTM_KEYWORD: &str = "ttm";
pub const LAYA_KEYWORD: &str = "laya";

pub struct Models {
    pub root: PathBuf,
    pub siglip: Option<SiglipModel>,
    pub tool_bank: Option<PromptBank>,
    pub mold_bank: Option<PromptBank>,
    pub ttm: Option<TtmForecaster>,
    pub laya: Option<LayaAdvisor>,
    pub dirs: BTreeMap<String, String>,
    pub errors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelSlot {
    pub key: String,
    pub label: String,
    pub dir: Option<String>,
    pub loaded: bool,
    pub detail: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStatus {
    pub root: String,
    pub slots: Vec<ModelSlot>,
    #[serde(default)]
    pub device: String,
}

fn child_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn locate(root: &Path, keyword: &str, marker: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let name_of = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut out = Vec::new();
    let push = |d: PathBuf, out: &mut Vec<PathBuf>| {
        if marker(&d) && crate::ml::ensure_safetensors_only(&d).is_ok() && !out.contains(&d) {
            out.push(d);
        }
    };
    if name_of(root).contains(keyword) {
        push(root.to_path_buf(), &mut out);
        for c in child_dirs(root) {
            push(c, &mut out);
        }
    }
    for c in child_dirs(root) {
        if !name_of(&c).contains(keyword) {
            continue;
        }
        push(c.clone(), &mut out);
        for g in child_dirs(&c) {
            push(g, &mut out);
        }
    }
    out
}

impl Models {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            siglip: None,
            tool_bank: None,
            mold_bank: None,
            ttm: None,
            laya: None,
            dirs: BTreeMap::new(),
            errors: BTreeMap::new(),
        }
    }

    pub fn set_root(&mut self, root: &Path) {
        *self = Self::new(root);
    }

    pub fn is_loaded(&self, key: &str) -> bool {
        match key {
            "siglip" => self.siglip.is_some(),
            "ttm" => self.ttm.is_some(),
            "laya" => self.laya.is_some(),
            _ => false,
        }
    }

    pub fn unload(&mut self, key: &str) -> bool {
        let was = self.is_loaded(key);
        match key {
            "siglip" => {
                self.siglip = None;
                self.tool_bank = None;
                self.mold_bank = None;
            }
            "ttm" => self.ttm = None,
            "laya" => self.laya = None,
            _ => {}
        }
        self.dirs.remove(key);
        self.errors.remove(key);
        was
    }

    pub fn unload_and_flush(&mut self, key: &str) -> bool {
        let was = self.unload(key);
        if was {
            crate::ml::flush_device();
        }
        was
    }

    pub fn flush_after_job(&mut self, keys: &[&str]) {
        let mut any = false;
        for key in keys {
            if self.unload(key) {
                any = true;
            }
        }
        if any {
            crate::ml::flush_device();
        }
    }

    pub fn detected_dir(&self, key: &str) -> Option<PathBuf> {
        match key {
            "siglip" => locate(&self.root, SIGLIP_KEYWORD, &|d: &Path| d.join("config.json").exists()).into_iter().next(),
            "ttm" => locate(&self.root, TTM_KEYWORD, &|d: &Path| d.join("config.json").exists()).into_iter().next(),
            "laya" => locate(&self.root, LAYA_KEYWORD, &|d: &Path| d.join("encoder").join("config.json").exists()).into_iter().next(),
            _ => None,
        }
    }

    pub fn load(&mut self, key: &str) -> Result<String, String> {
        match key {
            "siglip" => self.load_siglip(),
            "ttm" => self.load_ttm(),
            "laya" => self.load_laya(),
            other => Err(format!("알 수 없는 모델: {}", other)),
        }
    }

    pub fn load_siglip(&mut self) -> Result<String, String> {
        let found = locate(&self.root, SIGLIP_KEYWORD, &|d: &Path| d.join("config.json").exists());
        let dir = found.into_iter().next().ok_or_else(|| {
            format!(
                "{} 아래에서 이름에 'siglip' 이 들어간 safetensors 모델 폴더를 찾지 못했습니다 (예: siglip2-large-patch16-512)",
                self.root.display()
            )
        })?;
        let result = SiglipModel::load(&dir, &crate::ml::device());
        match result {
            Ok(m) => {
                let mut detail = format!(
                    "{}px · patch {}×{} · dim {} · text {}토큰",
                    m.image_size(),
                    m.grid(),
                    m.grid(),
                    m.hidden(),
                    m.max_text_len()
                );
                if m.tokenizer.is_some() {
                    self.tool_bank = Some(PromptBank::build(&m, wear::TOOL_PROMPTS)?);
                    self.mold_bank = Some(PromptBank::build(&m, wear::MOLD_PROMPTS)?);
                    detail.push_str(&format!(" · 영어 프롬프트 {}+{}종", wear::TOOL_PROMPTS.len(), wear::MOLD_PROMPTS.len()));
                } else {
                    self.tool_bank = None;
                    self.mold_bank = None;
                    detail.push_str(" · tokenizer.json 없음: 제로샷 유형 판정 없이 벡터 비교만 수행");
                }
                self.siglip = Some(m);
                self.dirs.insert("siglip".into(), dir.display().to_string());
                self.errors.remove("siglip");
                Ok(detail)
            }
            Err(e) => {
                self.errors.insert("siglip".into(), e.clone());
                Err(e)
            }
        }
    }

    pub fn load_ttm(&mut self) -> Result<String, String> {
        let found = locate(&self.root, TTM_KEYWORD, &|d: &Path| d.join("config.json").exists());
        if found.is_empty() {
            return Err(format!(
                "{} 아래에서 이름에 'ttm' 이 들어간 safetensors 모델 폴더를 찾지 못했습니다 (예: granite-timeseries-ttm-r3/52-16-dec-52-r3)",
                self.root.display()
            ));
        }
        let mut ranked: Vec<(usize, PathBuf)> = found
            .into_iter()
            .map(|d| {
                let ctx = crate::ml::read_json(&d.join("config.json"))
                    .ok()
                    .and_then(|v| v.get("context_length").and_then(|x| x.as_u64()))
                    .unwrap_or(u64::MAX) as usize;
                (ctx, d)
            })
            .collect();
        ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut last_err = String::new();
        for (_, dir) in ranked {
            match TtmForecaster::load(&dir, &crate::ml::device()) {
                Ok(m) => {
                    let detail = format!(
                        "context {} → 예측 {} · 분위 {}개 · {}",
                        m.context_length(),
                        m.prediction_length(),
                        m.config.quantile_levels.len(),
                        if m.config.decompose { "추세/잔차 분해형" } else { "단일형" }
                    );
                    self.ttm = Some(m);
                    self.dirs.insert("ttm".into(), dir.display().to_string());
                    self.errors.remove("ttm");
                    return Ok(detail);
                }
                Err(e) => last_err = format!("{}: {}", dir.display(), e),
            }
        }
        self.errors.insert("ttm".into(), last_err.clone());
        Err(last_err)
    }

    pub fn load_laya(&mut self) -> Result<String, String> {
        let found = locate(&self.root, LAYA_KEYWORD, &|d: &Path| d.join("encoder").join("config.json").exists());
        let dir = found.into_iter().next().ok_or_else(|| {
            format!(
                "{} 아래에서 이름에 'laya' 가 들어가고 encoder/config.json 이 있는 safetensors 모델 폴더를 찾지 못했습니다",
                self.root.display()
            )
        })?;
        match LayaAdvisor::load(&dir, &crate::ml::device()) {
            Ok(m) => {
                if !m.has_tokenizer() {
                    let e = "laya tokenizer.json 이 없어 상태 문장을 인코딩할 수 없습니다".to_string();
                    self.errors.insert("laya".into(), e.clone());
                    return Err(e);
                }
                self.laya = Some(m);
                self.dirs.insert("laya".into(), dir.display().to_string());
                self.errors.remove("laya");
                Ok("ModernBERT-large 인코더 + 결정 헤드 · 7개 조치 선택형 조언".into())
            }
            Err(e) => {
                self.errors.insert("laya".into(), e.clone());
                Err(e)
            }
        }
    }

    pub fn status(&self) -> ModelStatus {
        let slot = |key: &str, label: &str, loaded: bool, detail: String| ModelSlot {
            key: key.into(),
            label: label.into(),
            dir: self.dirs.get(key).cloned(),
            loaded,
            detail,
            error: self.errors.get(key).cloned(),
        };
        ModelStatus {
            root: self.root.display().to_string(),
            device: crate::ml::device_label(&crate::ml::device()),
            slots: vec![
                slot(
                    "siglip",
                    "SigLIP2 large patch16 512 (영어) — 공구·금형 이미지 형상 벡터",
                    self.siglip.is_some(),
                    self.siglip
                        .as_ref()
                        .map(|m| format!("{}px · {}×{} 패치 · dim {}", m.image_size(), m.grid(), m.grid(), m.hidden()))
                        .unwrap_or_else(|| "미로드".into()),
                ),
                slot(
                    "ttm",
                    "Granite TTM-R3 — 마모·부하 시계열 예측",
                    self.ttm.is_some(),
                    self.ttm
                        .as_ref()
                        .map(|m| format!("context {} → {}", m.context_length(), m.prediction_length()))
                        .unwrap_or_else(|| "미로드 (Holt 감쇠 추세로 대체)".into()),
                ),
                slot(
                    "laya",
                    "laya-typed-decisions — 판단 조언(상향만 허용)",
                    self.laya.is_some(),
                    if self.laya.is_some() { "로드됨".into() } else { "미로드 (결정적 게이트만 사용)".into() },
                ),
            ],
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IngestOptions {
    #[serde(default)]
    pub hint: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub view: Option<String>,
    #[serde(default)]
    pub px_per_mm: Option<f64>,
    #[serde(default)]
    pub tip: Option<String>,
    #[serde(default)]
    pub same_setup: Option<bool>,
    #[serde(default)]
    pub mm_per_px: Option<f64>,
    #[serde(default)]
    pub depth_range_mm: Option<f64>,
    #[serde(default)]
    pub depth_invert: Option<bool>,
    #[serde(default)]
    pub origin_x: Option<f64>,
    #[serde(default)]
    pub origin_y: Option<f64>,
    #[serde(default)]
    pub cell_mm: Option<f64>,
    #[serde(default)]
    pub tool_id: Option<String>,
    #[serde(default)]
    pub mold_id: Option<String>,
    #[serde(default)]
    pub cut_minutes: Option<f64>,
    #[serde(default)]
    pub hardness_hrc: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocEntry {
    pub name: String,
    pub kind: DocKind,
    pub kind_label: String,
    pub completeness: f64,
    pub required: Vec<String>,
    pub missing: Vec<String>,
    pub issues: Vec<Issue>,
    pub fields: BTreeMap<String, FieldValue>,
    pub applied: Vec<String>,
    pub duplicate: bool,
    pub at: i64,
    #[serde(default)]
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageView {
    pub key: String,
    pub domain: String,
    pub view: String,
    pub png_b64: String,
    pub grid: usize,
    pub zero_shot: Vec<PromptScore>,
    pub zone_of_patch: Vec<String>,
    pub side: Option<SideProfile>,
    pub end: Option<EndProfile>,
    pub notes: Vec<String>,
}

fn image_view(r: &ImageRecord) -> ImageView {
    ImageView {
        key: r.key.clone(),
        domain: r.domain.clone(),
        view: r.view.key().into(),
        png_b64: r.aligned_png_b64.clone(),
        grid: r.grid,
        zero_shot: r.zero_shot.clone(),
        zone_of_patch: r.zone_of_patch.clone(),
        side: r.side.clone(),
        end: r.end.clone(),
        notes: r.notes.clone(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageOutcome {
    pub role: String,
    pub image: ImageView,
    pub reference: Option<ImageView>,
    pub comparison: Option<WearComparison>,
    pub identity: Option<f32>,
    pub reliable: bool,
    pub type_confidence: Option<String>,
    pub measured_radial_um: Option<f64>,
    pub predicted_radial_um: Option<f64>,
    pub calibration_ratio: Option<f64>,
    pub radial_limit_um: Option<f64>,
    pub sds: Vec<String>,
    #[serde(default)]
    pub minutes: Option<f64>,
    #[serde(skip)]
    pub global: Vec<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftItem {
    pub axis: String,
    pub value: f64,
    pub mean: f64,
    pub sd: f64,
    pub n: u64,
    pub z: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoldGeometryOutcome {
    pub role: String,
    pub mold_id: String,
    pub descriptor: MoldDescriptor,
    pub vector: MoldVector,
    pub heightmap: HeightmapView,
    pub comparison: Option<MoldComparison>,
    pub fit: Option<DeviationFit>,
    pub fit_error: Option<String>,
    pub drift: Vec<DriftItem>,
    pub sds: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallOutcome {
    pub fit: WallFit,
    pub sds: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestOutcome {
    pub doc: DocEntry,
    pub tool_image: Option<ImageOutcome>,
    pub mold_image: Option<ImageOutcome>,
    pub mold_geometry: Option<MoldGeometryOutcome>,
    pub wall: Option<WallOutcome>,
    pub sds: Vec<String>,
    #[serde(default)]
    pub calibrations: Vec<CalibEvent>,
    #[serde(default)]
    pub library: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoolantRow {
    pub key: String,
    pub label: String,
    pub friction_mu: f64,
    pub interface_c: f64,
    pub reduction_pct: f64,
    pub tool_body_c: f64,
    pub crack_risk: f64,
    pub bue_risk: f64,
    pub vb_rate_mm_per_min: f64,
    pub tool_life_min: Option<f64>,
    pub wall_error_um: f64,
    pub feasible: bool,
    pub reasons: Vec<String>,
    pub recommended: bool,
    pub current: bool,
    #[serde(default)]
    pub dominant_wear: String,
    #[serde(default)]
    pub lubricant_access: f64,
    #[serde(default)]
    pub boiling_state: String,
    #[serde(default)]
    pub compat: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessOutcome {
    pub profile_name: String,
    pub purpose: String,
    pub source: String,
    pub analysis: CutAnalysis,
    pub calibration: Vec<CalibrationApplied>,
    pub sim: LoadSimReport,
    pub metadata: GCodeMetadata,
    pub coolant_matrix: Vec<CoolantRow>,
    pub nominal_mold: Option<MoldGeometryOutcome>,
    pub refit: Option<DeviationFit>,
    #[serde(default)]
    pub library: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptiveProgram {
    pub program_name: String,
    pub text: String,
    pub scaled_segments: usize,
    pub min_scale: f64,
    pub time_base_min: f64,
    pub time_adapted_min: f64,
    pub metadata: GCodeMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesInfo {
    pub key: String,
    pub unit: String,
    pub t_unit: String,
    pub points: usize,
    pub last: Option<f64>,
    pub episodes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub profile_name: String,
    pub tool: String,
    pub tool_signature: String,
    pub tool_id: String,
    pub material: String,
    pub material_scope: String,
    pub coolant: String,
    pub purpose: String,
    pub pattern_key: String,
    pub program: Option<String>,
    pub mold_id: String,
    pub cut_minutes: f64,
    pub rpm: u32,
    pub feed_mm_min: f64,
    pub ap_mm: f64,
    pub ae_mm: f64,
    pub tolerance_mm: f64,
    pub docs: Vec<DocEntry>,
    pub series: Vec<SeriesInfo>,
    pub tool_reference: Vec<String>,
    pub mold_reference: bool,
    pub has_nominal_mold: bool,
    pub has_measured_mold: bool,
    pub has_sim: bool,
    pub has_forecast: bool,
    pub has_decision: bool,
    pub calibration: Vec<CalibrationApplied>,
    #[serde(default)]
    pub frf: FrfSetting,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FrfSetting {
    pub fn_hz: Option<f64>,
    pub k_n_per_um: Option<f64>,
    pub zeta: f64,
    pub measured_stickout_mm: Option<f64>,
    pub stickout_mm: f64,
    pub estimated_fn_hz: f64,
    pub estimated_k_n_per_um: f64,
    pub applies: bool,
}

fn engaged_loss(g: &GeomWear, ap: f64, d: f64, view: ToolView) -> f64 {
    match view {
        ToolView::End => g.mean_loss_tip_um.max(0.0),
        ToolView::Side => {
            let z_lo = if g.flank_from_mm > 0.0 { g.flank_from_mm } else { (2.0 * g.corner_r_ref_mm).max(0.1 * d) };
            let z_hi = if ap > z_lo + 0.05 * d { ap } else { z_lo + 0.5 * d };
            let vals: Vec<f64> = g
                .z_mm
                .iter()
                .zip(g.radial_loss_um.iter())
                .filter(|(z, _)| **z >= z_lo && **z <= z_hi)
                .map(|(_, v)| v.max(0.0))
                .collect();
            if vals.is_empty() {
                g.mean_loss_tip_um.max(0.0)
            } else {
                vals.iter().sum::<f64>() / vals.len() as f64
            }
        }
    }
}

fn recency_weighted(pairs: &[(f64, f64, f64)]) -> (f64, f64) {
    let ages: Vec<f64> = pairs.iter().map(|p| p.0).collect();
    let hl = timeseries::derive_recency_half_life(&ages);
    let mut wm = 0.0;
    let mut wp = 0.0;
    for (age, m, p) in pairs.iter() {
        let w = hl.map(|h| timeseries::recency_weight(*age, h)).unwrap_or(1.0);
        wm += w * m;
        wp += w * p;
    }
    (wm, wp)
}

pub struct Workspace {
    pub data_dir: PathBuf,
    pub profile: MachiningProfile,
    pub purpose: Purpose,
    pub pattern_key: String,
    pub program: Option<(String, Vec<ToolPathSegment>)>,
    pub tool_id: String,
    pub mold_id: String,
    pub cut_minutes: f64,
    pub docs: Vec<DocEntry>,
    pub series: BTreeMap<String, Series>,
    pub analysis: Option<CutAnalysis>,
    pub calibration: Vec<CalibrationApplied>,
    pub sim: Option<LoadSimReport>,
    pub tool_wear: Option<WearComparison>,
    pub mold_wear: Option<WearComparison>,
    pub nominal_mold: Option<HeightmapView>,
    pub measured_mold: Option<HeightmapView>,
    pub nominal_vec: Option<(MoldDescriptor, MoldVector)>,
    pub measured_vec: Option<(MoldDescriptor, MoldVector)>,
    pub mold_fit: Option<DeviationFit>,
    pub wall_points: Vec<WallPoint>,
    pub wall_fit: Option<WallFit>,
    pub forecast: Option<ForecastResult>,
    pub decision: Option<DecisionReport>,
    pub last_state: Option<String>,
    seen: BTreeSet<String>,
    pending_calib: Vec<CalibEvent>,
}

impl Workspace {
    pub fn new(data_dir: &Path, profile: MachiningProfile) -> Self {
        let seen: BTreeSet<String> = std::fs::read_to_string(data_dir.join("seen.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let purpose = default_purpose(&profile);
        Self {
            data_dir: data_dir.to_path_buf(),
            purpose,
            profile,
            pattern_key: "pocket_zigzag".into(),
            program: None,
            tool_id: String::new(),
            mold_id: "mold".into(),
            cut_minutes: 0.0,
            docs: Vec::new(),
            series: BTreeMap::new(),
            analysis: None,
            calibration: Vec::new(),
            sim: None,
            tool_wear: None,
            mold_wear: None,
            nominal_mold: None,
            measured_mold: None,
            nominal_vec: None,
            measured_vec: None,
            mold_fit: None,
            wall_points: Vec::new(),
            wall_fit: None,
            forecast: None,
            decision: None,
            last_state: None,
            seen,
            pending_calib: Vec::new(),
        }
    }

    pub fn tool_key(&self) -> String {
        if self.tool_id.is_empty() {
            self.profile.endmill_setting.signature()
        } else {
            self.tool_id.clone()
        }
    }

    pub fn threshold(&self, key: &str) -> Option<f64> {
        let ctx = CutContext::from_profile(&self.profile);
        self.threshold_for(key, &ctx)
    }

    pub fn extend_seen<I: IntoIterator<Item = String>>(&mut self, it: I) {
        self.seen.extend(it);
    }

    pub fn replace_series(&mut self, series: BTreeMap<String, Series>) {
        self.series = series;
        self.forecast = None;
    }

    fn calib_event(&mut self, axis: &str, measured: f64, predicted: f64, source: &str) {
        self.pending_calib.push(CalibEvent {
            axis: axis.into(),
            measured,
            predicted,
            source: source.into(),
        });
    }

    fn vectors_dir(&self) -> PathBuf {
        self.data_dir.join("vectors")
    }

    fn mark_seen(&mut self, fp: &str) -> bool {
        let fresh = self.seen.insert(fp.to_string());
        if fresh {
            let _ = std::fs::create_dir_all(&self.data_dir);
            if let Ok(t) = serde_json::to_string(&self.seen) {
                let _ = std::fs::write(self.data_dir.join("seen.json"), t);
            }
        }
        fresh
    }

    fn invalidate_process(&mut self) {
        self.analysis = None;
        self.sim = None;
        self.decision = None;
        self.calibration.clear();
    }

    pub fn set_profile(&mut self, p: MachiningProfile) {
        let tool_changed = p.endmill_setting.signature() != self.profile.endmill_setting.signature();
        self.purpose = default_purpose(&p);
        self.profile = p;
        self.invalidate_process();
        self.nominal_mold = None;
        self.nominal_vec = None;
        self.mold_fit = None;
        self.wall_fit = None;
        if tool_changed {
            self.tool_wear = None;
            self.last_state = None;
        }
    }

    pub fn set_environment(&mut self, env: &crate::profile::ShopEnvironment) {
        if &self.profile.machine.environment != env {
            self.profile.machine.environment = env.clone();
            self.invalidate_process();
        }
    }

    pub fn set_purpose(&mut self, key: &str) {
        self.purpose = purpose_from_key(key);
        self.decision = None;
    }

    pub fn set_pattern(&mut self, key: &str) -> Result<(), String> {
        ToolPathPattern::from_key(key, &self.profile)?;
        self.pattern_key = key.to_string();
        self.program = None;
        self.sim = None;
        self.decision = None;
        Ok(())
    }

    pub fn set_ids(&mut self, tool_id: Option<String>, mold_id: Option<String>, cut_minutes: Option<f64>) {
        if let Some(t) = tool_id {
            self.tool_id = t.trim().to_string();
        }
        if let Some(m) = mold_id.filter(|m| !m.trim().is_empty()) {
            if m.trim() != self.mold_id {
                self.mold_id = m.trim().to_string();
                self.measured_mold = None;
                self.measured_vec = None;
                self.mold_fit = None;
                self.mold_wear = None;
            }
        }
        if let Some(c) = cut_minutes.filter(|c| *c >= 0.0) {
            self.cut_minutes = c;
        }
    }

    pub fn clear_program(&mut self) {
        self.program = None;
        self.sim = None;
    }

    pub fn frf_setting(&self) -> FrfSetting {
        let ctx = CutContext::from_profile(&self.profile);
        let m = &self.profile.machine;
        let est = crate::dynamics::estimated_modal(&ctx.tool, self.profile.conditions.axial_doc_mm.max(0.1), 1.0);
        FrfSetting {
            fn_hz: m.tip_fn_hz,
            k_n_per_um: m.tip_stiffness_n_per_um,
            zeta: m.damping_ratio,
            measured_stickout_mm: m.tip_frf_stickout_mm,
            stickout_mm: ctx.tool.stickout_mm,
            estimated_fn_hz: est.fn_hz,
            estimated_k_n_per_um: est.k_n_per_um,
            applies: ctx.tool.measured_fn_hz.is_some() && ctx.tool.measured_k_n_per_um.is_some(),
        }
    }

    pub fn set_frf(&mut self, fn_hz: Option<f64>, k_n_per_um: Option<f64>, zeta: Option<f64>, clear: bool) -> Result<(), String> {
        if let Some(z) = zeta {
            if !(z.is_finite() && (0.005..=0.2).contains(&z)) {
                return Err("감쇠비는 0.005~0.2 범위로 입력하세요 (예: 0.03 = 3%)".into());
            }
        }
        let pair = match (fn_hz, k_n_per_um) {
            (Some(f), Some(k)) if f.is_finite() && k.is_finite() && (100.0..=50000.0).contains(&f) && (0.05..=500.0).contains(&k) => Some((f, k)),
            (None, None) => None,
            _ => return Err("고유진동수(100~50000 Hz)와 강성(0.05~500 N/µm)을 함께 입력하세요".into()),
        };
        let stickout = CutContext::from_profile(&self.profile).tool.stickout_mm;
        let tool_key = self.profile.endmill_setting.dynamic_key();
        let m = &mut self.profile.machine;
        if clear {
            m.tip_fn_hz = None;
            m.tip_stiffness_n_per_um = None;
            m.tip_frf_stickout_mm = None;
            m.tip_frf_tool = None;
        } else if let Some((f, k)) = pair {
            m.tip_fn_hz = Some(f);
            m.tip_stiffness_n_per_um = Some(k);
            m.tip_frf_stickout_mm = Some(stickout);
            m.tip_frf_tool = Some(tool_key);
        }
        if let Some(z) = zeta {
            m.damping_ratio = z;
        }
        self.profile.modified_at = format!("{}", now_ms() / 1000);
        self.invalidate_process();
        Ok(())
    }

    fn segments(&self) -> Result<(String, Vec<ToolPathSegment>), String> {
        if let Some((name, segs)) = &self.program {
            return Ok((name.clone(), segs.clone()));
        }
        let pattern = ToolPathPattern::from_key(&self.pattern_key, &self.profile)?;
        Ok((
            pattern.key().to_string(),
            GCodeGenerator::generate_synthetic_segments_with_pattern(&self.profile, &pattern),
        ))
    }

    pub fn context(&self, sds: &SdsStore) -> (CutContext, Vec<CalibrationApplied>) {
        calibrated_context(&self.profile, Some(sds))
    }

    fn applied_factor(&self, axis: &str) -> f64 {
        self.calibration
            .iter()
            .find(|c| c.axis == axis)
            .map(|c| c.factor)
            .unwrap_or(1.0)
    }

    pub fn ensure_analysis(&mut self, sds: &SdsStore) -> CutAnalysis {
        if let Some(a) = &self.analysis {
            return a.clone();
        }
        let (ctx, calib) = self.context(sds);
        let body = ThermalBody::of_setup(&self.profile.workpiece_setup, &ctx.wp);
        let a = physics::analyze_cut(&ctx, &self.profile.conditions, true, body);
        self.calibration = calib;
        self.analysis = Some(a.clone());
        a
    }

    fn radial_limit_um(&self, ctx: &CutContext) -> f64 {
        match self.purpose {
            Purpose::Finishing => (0.5 * ctx.tolerance_mm * 1000.0).max(5.0),
            Purpose::Roughing => (ctx.vb_limit_mm() * ctx.tool.clearance_deg.to_radians().tan() * 1000.0).max(10.0),
        }
    }

    fn threshold_for(&self, key: &str, ctx: &CutContext) -> Option<f64> {
        match key {
            "vb_mm" => Some(ctx.vb_limit_mm()),
            "radial_loss_um" => Some(self.radial_limit_um(ctx)),
            "deviation_um" => Some(ctx.tolerance_mm * 1000.0),
            "spindle_power_kw" => Some(self.profile.machine.available_power_kw(self.profile.conditions.spindle_rpm as f64)),
            "spindle_load" => Some(100.0),
            "edge_z" => Some(4.5),
            _ => None,
        }
    }

    fn push_point(&mut self, key: &str, unit: &str, t_unit: &str, t: f64, y: f64) {
        let s = self
            .series
            .entry(key.to_string())
            .or_insert_with(|| Series::new(key, unit).with_time_unit(t_unit));
        if s.t_unit != t_unit {
            *s = Series::new(key, unit).with_time_unit(t_unit);
        }
        if let Some(pos) = s.t.iter().position(|v| (*v - t).abs() < 1e-9) {
            s.t.remove(pos);
            s.y.remove(pos);
        }
        s.push(t, y);
    }

    pub fn coolant_matrix(&self, sds: &SdsStore) -> Vec<CoolantRow> {
        let methods = [
            CoolantMethod::Dry,
            CoolantMethod::AirBlast,
            CoolantMethod::Mist,
            CoolantMethod::Flood,
            CoolantMethod::ThroughTool,
        ];
        let mut rows: Vec<CoolantRow> = methods
            .iter()
            .map(|m| {
                let mut p = self.profile.clone();
                p.coolant_config = CoolantConfig::from_method(m);
                p.conditions.coolant = coolant_type_of(m);
                let (ctx, _) = calibrated_context(&p, Some(sds));
                let body = ThermalBody::of_setup(&p.workpiece_setup, &ctx.wp);
                let a = physics::analyze_cut(&ctx, &p.conditions, true, body);
                let mut reasons = Vec::new();
                let lim = 0.85 * ctx.tool.coating.max_temp_c;
                if a.thermal.interface_c > lim {
                    reasons.push(format!("날끝 {:.0}°C > 코팅 허용 {:.0}°C", a.thermal.interface_c, lim));
                }
                if a.thermal.thermal_crack_risk > 0.45 {
                    reasons.push(format!("열균열 지수 {:.2}", a.thermal.thermal_crack_risk));
                }
                if a.thermal.bue_risk > 0.3 {
                    reasons.push(format!("구성인선 지수 {:.2}", a.thermal.bue_risk));
                }
                if let Some(tl) = ctx.wp.temp_limit_c {
                    if a.thermal.interface_c > tl {
                        reasons.push(format!("피삭재 허용 {:.0}°C 초과", tl));
                    }
                }
                let compat = &a.tribology.compat;
                if compat.severity >= 2 && compat.coolant_hint.is_some() {
                    reasons.extend(compat.messages.iter().cloned());
                }
                let dominant = a
                    .wear
                    .mechanisms
                    .iter()
                    .find(|x| x.key == a.wear.dominant)
                    .map(|x| format!("{} {:.0}%", x.label, x.share * 100.0))
                    .unwrap_or_default();
                CoolantRow {
                    key: m.key().into(),
                    label: m.label(),
                    friction_mu: a.coeffs.mu,
                    interface_c: a.thermal.interface_c,
                    reduction_pct: a.thermal.coolant_reduction_pct,
                    tool_body_c: a.thermal.tool_body_c,
                    crack_risk: a.thermal.thermal_crack_risk,
                    bue_risk: a.thermal.bue_risk,
                    vb_rate_mm_per_min: a.wear.vb_rate_mm_per_min,
                    tool_life_min: if a.wear.tool_life_min.is_finite() { Some(a.wear.tool_life_min) } else { None },
                    wall_error_um: a.error_budget_um.total_um,
                    feasible: reasons.is_empty(),
                    reasons,
                    recommended: false,
                    current: *m == self.profile.coolant_config.method,
                    dominant_wear: dominant,
                    lubricant_access: a.coeffs.lubricant_access,
                    boiling_state: a.thermal.cooling.boiling_state.clone(),
                    compat: compat.messages.clone(),
                }
            })
            .collect();
        let best = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.feasible)
            .max_by(|a, b| {
                let la = a.1.tool_life_min.unwrap_or(1e9);
                let lb = b.1.tool_life_min.unwrap_or(1e9);
                la.partial_cmp(&lb)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(b.1.wall_error_um.partial_cmp(&a.1.wall_error_um).unwrap_or(std::cmp::Ordering::Equal))
            })
            .map(|(i, _)| i);
        if let Some(i) = best {
            rows[i].recommended = true;
        }
        rows
    }

    pub fn run_process(&mut self, sds: &SdsStore) -> Result<ProcessOutcome, String> {
        self.profile.endmill_setting.validate()?;
        let (ctx, calib) = self.context(sds);
        let body = ThermalBody::of_setup(&self.profile.workpiece_setup, &ctx.wp);
        let analysis = physics::analyze_cut(&ctx, &self.profile.conditions, true, body);
        let (label, segs) = self.segments()?;
        if segs.is_empty() {
            return Err("시뮬레이션할 이동 명령이 없습니다".into());
        }
        let sim = loadsim::simulate_segments(&self.profile, &segs, &label, &ctx);
        let metadata = GCodeGenerator::metadata_for_segments(&self.profile, &segs, self.profile.machine.rapid_mm_min);
        self.calibration = calib.clone();
        self.analysis = Some(analysis.clone());
        let coolant_matrix = self.coolant_matrix(sds);
        let nominal = sim.heightmap.clone();
        let desc = mold::describe(&nominal, ctx.tool.radius(), ctx.tool.stickout_mm);
        let vec = mold::mold_vector(&desc);
        self.nominal_mold = Some(nominal.clone());
        self.nominal_vec = Some((desc.clone(), vec.clone()));
        self.sim = Some(sim.clone());
        self.decision = None;
        let comparison = self.measured_vec.as_ref().map(|(_, m)| mold::compare(&vec, m));
        let refit = match &self.measured_mold {
            Some(measured) => {
                let pred = MoldPrediction::from_sim(&sim, ctx.wp.expansion);
                match mold::deviation_fit(measured, &nominal, &pred) {
                    Ok(mut f) => {
                        f.residual_map = shrink_heightmap(&f.residual_map, 160);
                        self.mold_fit = Some(f.clone());
                        Some(f)
                    }
                    Err(_) => None,
                }
            }
            None => None,
        };
        Ok(ProcessOutcome {
            profile_name: self.profile.name.clone(),
            purpose: purpose_key(self.purpose).into(),
            source: match &self.program {
                Some((n, _)) => format!("NC 프로그램: {}", n),
                None => format!("패턴: {}", label),
            },
            analysis,
            calibration: calib,
            sim: trim_sim(&sim),
            metadata,
            coolant_matrix,
            nominal_mold: Some(MoldGeometryOutcome {
                role: "nominal".into(),
                mold_id: self.mold_id.clone(),
                descriptor: desc,
                vector: vec,
                heightmap: shrink_heightmap(&nominal, 160),
                comparison,
                fit: refit.clone(),
                fit_error: None,
                drift: Vec::new(),
                sds: Vec::new(),
            }),
            refit,
            library: Vec::new(),
        })
    }

    pub fn recommend(&mut self, sds: &SdsStore, apply: bool) -> Result<Recommendation, String> {
        let (ctx, _) = self.context(sds);
        let body = ThermalBody::of_setup(&self.profile.workpiece_setup, &ctx.wp);
        let rec = physics::recommend(&ctx, self.profile.endmill_setting.is_high_end, body, self.purpose);
        if apply {
            let mut c = rec.conditions.clone();
            c.coolant = coolant_type_of(&self.profile.coolant_config.method);
            self.profile.conditions = c;
            self.profile.modified_at = format!("{}", now_ms() / 1000);
            self.invalidate_process();
        }
        Ok(rec)
    }

    fn apply_document(&mut self, doc: &NormalizedDocument, sds: &mut SdsStore, fresh: bool, opt: &IngestOptions) -> (Vec<String>, Vec<String>) {
        let mut applied = Vec::new();
        let mut log = Vec::new();
        let blocking = doc.issues.iter().any(|i| i.level == "error");
        match doc.kind {
            DocKind::Profile => {
                if let (Some(p), false) = (doc.profile.clone(), blocking) {
                    applied.push(format!("활성 프로필 교체: {}", p.name));
                    self.set_profile(p);
                }
            }
            DocKind::ToolSpec => {
                if let (Some(t), false) = (doc.tool.clone(), blocking) {
                    let old = self.profile.endmill_setting.clone();
                    self.profile.endmill_setting = t.clone();
                    applied.push(format!("공구 교체: {} ({})", t.name, t.signature()));
                    if (old.diameter_mm - t.diameter_mm).abs() > 1e-6 || old.flute_count != t.flute_count || old.nose != t.nose {
                        let (ctx, _) = calibrated_context(&self.profile, Some(sds));
                        let body = ThermalBody::of_setup(&self.profile.workpiece_setup, &ctx.wp);
                        let rec = physics::recommend(&ctx, t.is_high_end, body, self.purpose);
                        let mut c = rec.conditions;
                        c.coolant = coolant_type_of(&self.profile.coolant_config.method);
                        self.profile.conditions = c;
                        applied.push("직경/날수/노즈가 바뀌어 절삭 조건을 물리 모델로 재산정".into());
                    }
                    self.invalidate_process();
                    self.tool_wear = None;
                }
            }
            DocKind::Workpiece => {
                if !blocking {
                    if let Some(w) = doc.workpiece.clone() {
                        self.profile.workpiece_setup = w;
                        applied.push("원소재 설정 교체".into());
                    } else if let Some(m) = doc.material.clone() {
                        let m = match (m, opt.hardness_hrc) {
                            (WorkpieceMaterial::AlloySteel { .. }, Some(h)) => WorkpieceMaterial::AlloySteel { hardness_hrc: h },
                            (m, _) => m,
                        };
                        self.profile.workpiece_setup.material = m.clone();
                        if let WorkpieceMaterial::AlloySteel { hardness_hrc } = m {
                            self.profile.workpiece_setup.hardness_hrc = Some(hardness_hrc);
                        }
                        applied.push(format!("피삭재 교체: {}", self.profile.workpiece_setup.material_label()));
                    }
                    if !applied.is_empty() {
                        let (ctx, _) = calibrated_context(&self.profile, Some(sds));
                        let body = ThermalBody::of_setup(&self.profile.workpiece_setup, &ctx.wp);
                        let rec = physics::recommend(&ctx, self.profile.endmill_setting.is_high_end, body, self.purpose);
                        let mut c = rec.conditions;
                        c.coolant = coolant_type_of(&self.profile.coolant_config.method);
                        self.profile.conditions = c;
                        applied.push("피삭재가 바뀌어 절삭 조건을 물리 모델로 재산정".into());
                        self.invalidate_process();
                    }
                }
            }
            DocKind::GCode => {
                if let (Some(segs), false) = (doc.segments.clone(), blocking) {
                    if let Some(g) = &doc.gcode {
                        if let Some(rpm) = g.spindle_rpm.filter(|r| *r > 0.0) {
                            let c = &mut self.profile.conditions;
                            let old = c.spindle_rpm.max(1) as f64;
                            c.spindle_rpm = rpm.round() as u32;
                            c.cutting_speed_m_min = std::f64::consts::PI * self.profile.endmill_setting.diameter_mm * rpm / 1000.0;
                            if (old - rpm).abs() > 0.5 {
                                applied.push(format!("회전수를 NC 의 S{} 로 맞춤 (이전 {:.0})", rpm, old));
                            }
                        }
                        if g.feed_max > 0.0 {
                            let z = self.profile.endmill_setting.flute_count.max(1) as f64;
                            let c = &mut self.profile.conditions;
                            c.feed_rate_mm_min = g.feed_max;
                            c.feed_per_tooth_mm = g.feed_max / (c.spindle_rpm.max(1) as f64 * z);
                            applied.push(format!("이송 기준을 NC 최대 F{:.0} 로 맞춤 (fz {:.4} mm)", g.feed_max, c.feed_per_tooth_mm));
                        }
                    }
                    applied.push(format!("NC 프로그램 {} 개 이동을 시뮬레이션 대상으로 지정", segs.len()));
                    self.program = Some((doc.name.clone(), segs));
                    self.invalidate_process();
                }
            }
            DocKind::MeasurementLog => {
                if !doc.series.is_empty() {
                    let keys: Vec<String> = doc.series.iter().map(|s| s.key.clone()).collect();
                    for s in doc.series.iter() {
                        self.series.insert(s.key.clone(), s.clone());
                    }
                    applied.push(format!("측정 시계열 {}종 등록: {}", keys.len(), keys.join(", ")));
                    if fresh {
                        log.extend(self.calibrate_from_series(sds, &keys));
                    } else {
                        log.push("이미 반영한 동일 로그라 보정 원장에 다시 기록하지 않았습니다".into());
                    }
                }
            }
            _ => {}
        }
        (applied, log)
    }

    fn calibrate_from_series(&mut self, sds: &mut SdsStore, keys: &[String]) -> Vec<String> {
        let mut log = Vec::new();
        let analysis = self.ensure_analysis(sds);
        let part_min = self.sim.as_ref().map(|s| s.cut_time_min);
        let scope = process_scope(&self.profile);
        if keys.iter().any(|k| k == "vb_mm") {
            if let Some(s) = self.series.get("vb_mm").cloned() {
                match s.minutes_per_t(part_min) {
                    Some(k) => {
                        let c = self.applied_factor("wear").max(1e-9);
                        let effective = analysis.effective_trajectory();
                        let traj = &effective;
                        let t_last = s.t.last().cloned().unwrap_or(0.0);
                        let mut pairs = Vec::new();
                        for (a, b) in timeseries::episodes_of(&s) {
                            let t0 = s.t[a];
                            let y0 = s.y[a];
                            for i in a..b {
                                let dt_min = (s.t[i] - t0) * k;
                                let pred = traj.growth_from(y0, dt_min / c);
                                let grown = s.y[i] - y0;
                                if dt_min > 0.0 && pred > 1e-5 && grown > 0.0 {
                                    pairs.push((t_last - s.t[i], grown, pred));
                                }
                            }
                        }
                        if pairs.len() >= 2 {
                            let (wm, wp) = recency_weighted(&pairs);
                            sds.record_calibration(&scope, "wear", wm, wp);
                            self.calib_event("wear", wm, wp, "vb_log");
                            log.push(format!(
                                "VB 로그 {}점(최근 가중) 측정/예측 = {:.2} → 공정 원장 'wear' 보정 기록 ({})",
                                pairs.len(),
                                wm / wp.max(1e-12),
                                scope.key_secondary()
                            ));
                        }
                    }
                    None => log.push("VB 로그의 시간축이 행 순번이라 물리 예측과 비교하지 못했습니다 (시간 또는 부품 수 열 필요)".into()),
                }
            }
        }
        let power_series = if keys.iter().any(|k| k == "spindle_power_kw") {
            self.series.get("spindle_power_kw").cloned()
        } else if keys.iter().any(|k| k == "spindle_load") {
            self.series.get("spindle_load").cloned().map(|mut s| {
                let rated = self.profile.machine.max_power_kw;
                for y in s.y.iter_mut() {
                    *y = *y / 100.0 * rated;
                }
                s
            })
        } else {
            None
        };
        if let Some(s) = power_series {
            let (ep, _) = timeseries::current_episode(&s);
            let t_last = ep.t.last().cloned().unwrap_or(0.0);
            let pred_base = analysis.spindle_power_kw / self.applied_factor("power");
            let pairs: Vec<(f64, f64, f64)> = ep
                .t
                .iter()
                .zip(ep.y.iter())
                .filter(|(_, y)| **y > 0.0)
                .map(|(t, y)| (t_last - t, *y, pred_base))
                .collect();
            if pairs.len() >= 2 && pred_base > 1e-3 {
                let (wm, wp) = recency_weighted(&pairs);
                sds.record_calibration(&scope, "power", wm, wp);
                self.calib_event("power", wm, wp, "power_log");
                log.push(format!(
                    "동력/부하 로그 {}점 측정/예측 = {:.2} → 공정 원장 'power' 보정 기록",
                    pairs.len(),
                    wm / wp.max(1e-12)
                ));
            }
        }
        if !log.is_empty() {
            self.invalidate_process();
        }
        log
    }

    fn type_confidence(sds: &mut SdsStore, scope: &Scope, axis: &str, scores: &[(String, f32)], record: bool) -> Option<String> {
        if scores.len() < 2 {
            return None;
        }
        let mut v = scores.to_vec();
        v.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let (a, sa) = (v[0].0.clone(), v[0].1);
        let (b, sb) = (v[1].0.clone(), v[1].1);
        if sa <= 0.02 {
            return Some("유형 변화 미미".into());
        }
        let margin = (sa - sb) as f64;
        let hist = sds.adaptive_decay(scope, axis);
        let (level, basis) = match hist {
            Some((m, sd, n)) if sd > 1e-9 => {
                let z = (margin - m) / sd;
                let lv = if z < -1.0 {
                    "낮음"
                } else if z > 1.0 {
                    "높음"
                } else {
                    "보통"
                };
                (lv, format!("동일 스코프 {}건 평균 차이 {:.3}±{:.3} 대비 z={:.1}", n, m, sd, z))
            }
            _ => {
                let lv = if margin < 0.03 {
                    "낮음"
                } else if margin > 0.1 {
                    "높음"
                } else {
                    "보통"
                };
                (lv, "이력 부족: 고정 기준 0.03/0.10 적용".to_string())
            }
        };
        if level == "낮음" && record {
            sds.record_confusion(scope, &a, &b, margin);
        }
        let past = sds
            .confusion_share(scope, &a, &b)
            .map(|(share, ties)| format!(", 과거 {}–{} 혼동 {}건({:.0}%)", a, b, ties, share * 100.0))
            .unwrap_or_default();
        Some(format!("{} — 1위 {} / 2위 {} 차이 {:.3} ({}{})", level, a, b, margin, basis, past))
    }

    pub fn tool_image(&mut self, models: &Models, sds: &mut SdsStore, bytes: &[u8], opt: &IngestOptions, record: bool) -> Result<ImageOutcome, String> {
        let model = models
            .siglip
            .as_ref()
            .ok_or_else(|| "SigLIP2 모델이 로드되지 않았습니다. 모델 폴더를 지정하고 'SigLIP2 로드' 를 먼저 실행하세요".to_string())?;
        let empty = PromptBank::empty();
        let bank = models.tool_bank.as_ref().unwrap_or(&empty);
        let view = ToolView::from_key(opt.view.as_deref().unwrap_or("side"));
        self.set_ids(opt.tool_id.clone(), None, None);
        let setting = self.profile.endmill_setting.clone();
        let dims = wear::tool_dims_from_setting(&setting);
        let sig = setting.signature();
        let key = if self.tool_id.is_empty() { sig.clone() } else { self.tool_id.clone() };
        let role = opt.role.clone().unwrap_or_else(|| "current".into()).to_lowercase();
        let rec = wear::analyze_tool_image(model, bank, bytes, view, &dims, &key, opt.px_per_mm, opt.tip.as_deref())?;
        let dir = self.vectors_dir();
        let mut log = Vec::new();
        if role == "reference" {
            wear::save_record(&dir, &rec)?;
            self.tool_wear = None;
            log.push(format!("기준 벡터 저장: tool.image / {} / {} (전역 1개 + 패치 {}개 + 공동공간 {}개)", key, view.key(), rec.patches.len(), rec.dense.len()));
            return Ok(ImageOutcome {
                role,
                image: image_view(&rec),
                reference: None,
                comparison: None,
                identity: None,
                reliable: true,
                type_confidence: None,
                measured_radial_um: None,
                predicted_radial_um: None,
                calibration_ratio: None,
                radial_limit_um: None,
                sds: log,
                minutes: Some(0.0),
                global: rec.global.clone(),
            });
        }
        let reference = wear::load_record(&dir, "tool.image", &key, view).map_err(|_| {
            format!(
                "'{}' ({}) 의 기준(신품) 이미지 벡터가 없습니다. 같은 공구 ID·뷰로 role=reference 를 먼저 등록하세요",
                key,
                view.key()
            )
        })?;
        let scope = Scope::new(Track::ToolImage, &sig, view.key());
        let baseline = sds.adaptive_baseline(&scope, "edge_top_distance").map(|(m, s, n)| (m as f32, s as f32, n));
        let analysis = self.ensure_analysis(sds);
        let (ctx, _) = self.context(sds);
        let lim = CompareLimits {
            radial_limit_um: self.radial_limit_um(&ctx),
            tool_diameter_mm: setting.diameter_mm,
            loc_mm: setting.loc_mm,
            flutes: setting.flute_count as u32,
            helix_deg: setting.helix_angle_deg,
            same_setup: opt.same_setup.unwrap_or(false),
        };
        let mut cmp = wear::compare(model, bank, &reference, &rec, &lim, baseline)?;
        let identity = 1.0 - cmp.global_distance;
        let id_z = sds
            .adaptive_baseline(&scope, "global_distance")
            .and_then(|(m, s, _)| if s > 1e-6 { Some((cmp.global_distance as f64 - m) / s) } else { None });
        let reliable = identity >= 0.6 && id_z.map(|z| z < 4.0).unwrap_or(true);
        if !reliable {
            cmp.notes.push(format!(
                "전역 형상 유사도 {:.2}{} — 다른 공구이거나 배율·조명·각도가 달라 부위 비교 신뢰도가 낮습니다. 보정 원장에는 기록하지 않습니다",
                identity,
                id_z.map(|z| format!(" (이력 대비 z={:.1})", z)).unwrap_or_default()
            ));
        }
        let type_confidence = Self::type_confidence(sds, &scope, "wear_type", &cmp.type_scores, record);
        let scores: Vec<f32> = cmp.type_scores.iter().map(|t| t.1).collect();
        if record {
            sds.record_decay(&scope, "wear_type", &scores);
        } else {
            log.push("이미 입력한 동일 이미지라 원장에 다시 기록하지 않았습니다".into());
        }
        let ap = self.profile.conditions.axial_doc_mm.min(setting.loc_mm);
        let measured = cmp.geometry.as_ref().map(|g| engaged_loss(g, ap, setting.diameter_mm, view));
        let minutes = opt.cut_minutes.filter(|m| *m > 0.0);
        if let Some(m) = minutes {
            self.cut_minutes = m;
        }
        let rad_per_vb = ctx.tool.clearance_deg.to_radians().tan() * 1000.0;
        let effective = analysis.effective_trajectory();
        let predicted = minutes.map(|m| effective.vb_at(m) * rad_per_vb);
        let mut ratio = None;
        if let (Some(meas), Some(pred), Some(m)) = (measured, predicted, minutes) {
            if record && reliable && meas >= 3.0 && pred >= 0.5 {
                let base = effective.vb_at(m / self.applied_factor("wear").max(1e-9)) * rad_per_vb;
                ratio = Some(meas / base.max(1e-9));
                let pscope = process_scope(&self.profile);
                sds.record_calibration(&pscope, "wear", meas, base);
                self.calib_event("wear", meas, base, "tool_image");
                let gscope = Scope::new(Track::ToolGeom, &sig, &material_scope_key(&self.profile.workpiece_setup.effective_material()));
                sds.record_baseline(&gscope, "radial_loss_rate_um_per_min", meas / m);
                log.push(format!(
                    "실루엣 반경 손실 {:.1} µm / 물리 예측(보정 전) {:.1} µm = {:.2} → 공정 원장 'wear' 기록 ({})",
                    meas,
                    base,
                    meas / base.max(1e-9),
                    pscope.key_secondary()
                ));
                self.invalidate_process();
            } else if meas < 3.0 {
                log.push(format!("반경 손실 {:.1} µm 는 실루엣 분해능(약 3 µm) 미만이라 보정에 쓰지 않았습니다", meas));
            }
        }
        if let Some(m) = minutes {
            if let Some(meas) = measured {
                self.push_point("radial_loss_um", "µm", "min", m, meas);
            }
            let edge = cmp.zones.iter().filter(|z| z.zone != "shank").map(|z| z.z).fold(0f32, f32::max) as f64;
            self.push_point("edge_z", "z", "min", m, edge);
        }
        if record && reliable && cmp.state == WearState::Normal {
            let edge_top = cmp.zones.iter().map(|z| z.top_distance).fold(0f32, f32::max) as f64;
            sds.record_baseline(&scope, "edge_top_distance", edge_top);
            sds.record_baseline(&scope, "global_distance", cmp.global_distance as f64);
            log.push(format!("정상 판정 → '{}' 잡음 기준선(edge/global) 갱신", scope.key_secondary()));
        }
        self.tool_wear = Some(cmp.clone());
        self.decision = None;
        Ok(ImageOutcome {
            role,
            image: image_view(&rec),
            reference: Some(image_view(&reference)),
            comparison: Some(cmp),
            identity: Some(identity),
            reliable,
            type_confidence,
            measured_radial_um: measured,
            predicted_radial_um: predicted,
            calibration_ratio: ratio,
            radial_limit_um: Some(lim.radial_limit_um),
            sds: log,
            minutes,
            global: rec.global.clone(),
        })
    }

    pub fn mold_image(&mut self, models: &Models, sds: &mut SdsStore, bytes: &[u8], opt: &IngestOptions, record: bool) -> Result<ImageOutcome, String> {
        let model = models
            .siglip
            .as_ref()
            .ok_or_else(|| "SigLIP2 모델이 로드되지 않았습니다. 모델 폴더를 지정하고 'SigLIP2 로드' 를 먼저 실행하세요".to_string())?;
        let empty = PromptBank::empty();
        let bank = models.mold_bank.as_ref().unwrap_or(&empty);
        self.set_ids(None, opt.mold_id.clone(), None);
        let mold_id = self.mold_id.clone();
        let role = opt.role.clone().unwrap_or_else(|| "current".into()).to_lowercase();
        let rec = wear::analyze_mold_image(model, bank, bytes, &mold_id)?;
        let dir = self.vectors_dir();
        let mut log = Vec::new();
        if role == "reference" {
            wear::save_record(&dir, &rec)?;
            self.mold_wear = None;
            log.push(format!("기준 표면 벡터 저장: mold.image / {}", mold_id));
            return Ok(ImageOutcome {
                role,
                image: image_view(&rec),
                reference: None,
                comparison: None,
                identity: None,
                reliable: true,
                type_confidence: None,
                measured_radial_um: None,
                predicted_radial_um: None,
                calibration_ratio: None,
                radial_limit_um: None,
                sds: log,
                minutes: None,
                global: rec.global.clone(),
            });
        }
        let reference = wear::load_record(&dir, "mold.image", &mold_id, ToolView::Side)
            .map_err(|_| format!("금형 '{}' 의 기준 표면 이미지가 없습니다. role=reference 로 먼저 등록하세요", mold_id))?;
        let scope = Scope::new(Track::MoldImage, &mold_id, "");
        let baseline = sds.adaptive_baseline(&scope, "surface_top_distance").map(|(m, s, n)| (m as f32, s as f32, n));
        let s = &self.profile.endmill_setting;
        let lim = CompareLimits {
            radial_limit_um: 1.0,
            tool_diameter_mm: s.diameter_mm,
            loc_mm: s.loc_mm,
            flutes: s.flute_count as u32,
            helix_deg: s.helix_angle_deg,
            same_setup: true,
        };
        let mut cmp = wear::compare(model, bank, &reference, &rec, &lim, baseline)?;
        let gz = sds
            .adaptive_baseline(&scope, "surface_global_distance")
            .and_then(|(m, sd, n)| if sd > 1e-6 && n >= 5 { Some((cmp.global_distance as f64 - m) / sd) } else { None });
        let defect = cmp
            .type_scores
            .iter()
            .filter(|(k, _)| k != "mirror")
            .map(|(_, d)| *d)
            .fold(0f32, f32::max);
        let mut rule = WearState::Normal;
        if defect >= 0.25 || gz.map(|z| z >= 4.5).unwrap_or(false) {
            rule = WearState::Alert;
        } else if defect >= 0.12 || gz.map(|z| z >= 3.0).unwrap_or(false) {
            rule = WearState::Watch;
        }
        if rule > WearState::Normal {
            cmp.state_reasons.push(format!(
                "표면 결함 프롬프트 증가 {:.2}{}",
                defect,
                gz.map(|z| format!(", 전역 거리 이력 대비 z={:.1}", z)).unwrap_or_default()
            ));
        }
        cmp.state = cmp.state.max(rule);
        let zone_top = cmp.zones.iter().map(|z| z.z).fold(0f32, f32::max);
        cmp.severity_index = (zone_top / 4.5)
            .max(defect / 0.25)
            .max(gz.map(|z| (z / 4.5) as f32).unwrap_or(0.0));
        let type_confidence = Self::type_confidence(sds, &scope, "surface_type", &cmp.type_scores, record);
        let scores: Vec<f32> = cmp.type_scores.iter().map(|t| t.1).collect();
        if record {
            sds.record_decay(&scope, "surface_type", &scores);
        }
        if record && cmp.state == WearState::Normal {
            let top = cmp.zones.iter().map(|z| z.top_distance).fold(0f32, f32::max) as f64;
            sds.record_baseline(&scope, "surface_top_distance", top);
            sds.record_baseline(&scope, "surface_global_distance", cmp.global_distance as f64);
            log.push(format!("정상 표면 → '{}' 기준선 갱신", scope.key_secondary()));
        }
        self.mold_wear = Some(cmp.clone());
        self.decision = None;
        Ok(ImageOutcome {
            role,
            image: image_view(&rec),
            reference: Some(image_view(&reference)),
            identity: Some(1.0 - cmp.global_distance),
            comparison: Some(cmp),
            reliable: true,
            type_confidence,
            measured_radial_um: None,
            predicted_radial_um: None,
            calibration_ratio: None,
            radial_limit_um: None,
            sds: log,
            minutes: None,
            global: rec.global.clone(),
        })
    }

    fn record_fit_calibration(&mut self, sds: &mut SdsStore, fit: &DeviationFit) -> Vec<String> {
        let mut log = Vec::new();
        let scope = process_scope(&self.profile);
        for c in fit.coefficients.iter() {
            if !c.identifiable || !c.stderr.is_finite() {
                continue;
            }
            if c.name == "axial_offset_um" {
                sds.record_baseline(&scope, "mold_axial_offset_um", c.value);
                continue;
            }
            let axis = match c.name.as_str() {
                "radial_wear_um" => "wear",
                "deflection_um" => "deflection",
                "workpiece_dT_c" => "temperature",
                _ => continue,
            };
            let significant = c.value.abs() > 2.0 * c.stderr;
            if let Some(p) = c.predicted {
                if p.abs() >= 0.5 && significant && c.value > 0.0 && p > 0.0 {
                    let base = p / self.applied_factor(axis);
                    sds.record_calibration(&scope, axis, c.value, base);
                    self.calib_event(axis, c.value, base, "mold_fit");
                    log.push(format!(
                        "금형 편차 분해 {}: 측정 {:.2} / 예측(보정 전) {:.2} = {:.2} → 공정 원장 '{}' 기록",
                        c.label,
                        c.value,
                        base,
                        c.value / base,
                        axis
                    ));
                }
            }
        }
        log
    }

    pub fn mold_geometry(&mut self, sds: &mut SdsStore, h: HeightmapView, role: &str, record: bool) -> Result<MoldGeometryOutcome, String> {
        if h.nx < 3 || h.ny < 3 {
            return Err("높이맵이 3×3 보다 작습니다".into());
        }
        let setting = self.profile.endmill_setting.clone();
        let desc = mold::describe(&h, setting.diameter_mm / 2.0, setting.effective_stickout_mm());
        let vec = mold::mold_vector(&desc);
        let mid = self.mold_id.clone();
        let scope = Scope::new(Track::MoldGeom, &mid, &material_scope_key(&self.profile.workpiece_setup.effective_material()));
        let axes = [
            ("max_depth_mm", desc.max_depth_mm),
            ("mean_depth_mm", desc.mean_depth_mm),
            ("steep_ratio", desc.steep_ratio),
            ("flat_ratio", desc.flat_ratio),
            ("min_concave_radius_mm", desc.min_concave_radius_mm),
            ("rest_material_ratio", desc.rest_material_ratio),
            ("removed_volume_cm3", desc.removed_volume_cm3),
        ];
        let mut drift = Vec::new();
        for (axis, v) in axes.iter() {
            if let Some((m, s, n)) = sds.adaptive_baseline(&scope, axis) {
                if s > 1e-9 {
                    drift.push(DriftItem {
                        axis: axis.to_string(),
                        value: *v,
                        mean: m,
                        sd: s,
                        n,
                        z: (v - m) / s,
                    });
                }
            }
        }
        let mut log = Vec::new();
        let role = if role == "nominal" { "nominal" } else { "measured" };
        if record && role == "measured" {
            for (axis, v) in axes.iter() {
                sds.record_baseline(&scope, axis, *v);
            }
            log.push(format!("측정 금형 형상 지표 {}종 → '{}' 기준선 기록", axes.len(), scope.key_secondary()));
        }
        if role == "nominal" {
            self.nominal_mold = Some(h.clone());
            self.nominal_vec = Some((desc.clone(), vec.clone()));
        } else {
            self.measured_mold = Some(h.clone());
            self.measured_vec = Some((desc.clone(), vec.clone()));
        }
        let comparison = match (&self.nominal_vec, &self.measured_vec) {
            (Some((_, a)), Some((_, b))) => Some(mold::compare(a, b)),
            _ => None,
        };
        let mut fit = None;
        let mut fit_error = None;
        if let (Some(measured), Some(nominal)) = (&self.measured_mold, &self.nominal_mold) {
            match &self.sim {
                Some(sim) => {
                    let (ctx, _) = self.context(sds);
                    let pred = MoldPrediction::from_sim(sim, ctx.wp.expansion);
                    match mold::deviation_fit(measured, nominal, &pred) {
                        Ok(mut f) => {
                            if record && role == "measured" {
                                log.extend(self.record_fit_calibration(sds, &f));
                            }
                            f.residual_map = shrink_heightmap(&f.residual_map, 160);
                            self.mold_fit = Some(f.clone());
                            fit = Some(f);
                        }
                        Err(e) => fit_error = Some(e),
                    }
                }
                None => fit_error = Some("편차를 마모·휨·열로 분해하려면 먼저 '공정 해석'(부하 시뮬레이션)을 실행하세요".into()),
            }
        }
        if !log.is_empty() {
            self.decision = None;
        }
        Ok(MoldGeometryOutcome {
            role: role.into(),
            mold_id: mid,
            descriptor: desc,
            vector: vec,
            heightmap: shrink_heightmap(&h, 160),
            comparison,
            fit,
            fit_error,
            drift,
            sds: log,
        })
    }

    pub fn wall_cmm(&mut self, sds: &mut SdsStore, pts: Vec<WallPoint>, record: bool) -> Result<WallOutcome, String> {
        let sim = self
            .sim
            .as_ref()
            .ok_or_else(|| "벽면 측정을 분해하려면 먼저 '공정 해석'(부하 시뮬레이션)을 실행하세요".to_string())?;
        let tol = (2.0 * sim.cell_mm).max(1.0);
        let fit = mold::wall_fit(&pts, &sim.wall_errors, tol)?;
        let mut log = Vec::new();
        if record {
            let scope = process_scope(&self.profile);
            for c in fit.coefficients.iter() {
                let axis = match c.name.as_str() {
                    "deflection_gain" => "deflection",
                    "wear_gain" => "wear",
                    "thermal_gain" => "temperature",
                    _ => continue,
                };
                if c.identifiable && c.value > 0.0 && c.stderr.is_finite() && c.value > 2.0 * c.stderr {
                    let raw = c.value * self.applied_factor(axis);
                    sds.record_calibration(&scope, axis, raw, 1.0);
                    self.calib_event(axis, raw, 1.0, "wall_cmm");
                    log.push(format!("벽면 CMM {}: 이득 {:.2} (보정 전 기준 {:.2}) → 공정 원장 '{}' 기록", c.label, c.value, raw, axis));
                }
            }
        }
        self.wall_points = pts;
        self.wall_fit = Some(fit.clone());
        self.decision = None;
        Ok(WallOutcome { fit, sds: log })
    }

    pub fn ingest(&mut self, models: &Models, sds: &mut SdsStore, name: &str, bytes: &[u8], opt: &IngestOptions) -> Result<IngestOutcome, String> {
        let lower = name.to_lowercase();
        let is_image = [".png", ".jpg", ".jpeg", ".bmp", ".tif", ".tiff", ".webp"].iter().any(|e| lower.ends_with(e));
        let geometry_role = matches!(opt.role.as_deref(), Some("nominal") | Some("measured"));
        let hint = opt.hint.as_deref().and_then(DocKind::from_key).or(if is_image && geometry_role {
            Some(DocKind::MoldDepth)
        } else {
            None
        });
        let doc = ingest::ingest(name, bytes, hint);
        let fp = fingerprint(&format!("{:?}|{}", doc.kind, opt.role.clone().unwrap_or_default()), bytes);
        let fresh = !self.seen.contains(&fp);
        self.pending_calib.clear();
        let mut out = IngestOutcome {
            doc: DocEntry {
                name: doc.name.clone(),
                kind: doc.kind,
                kind_label: doc.kind_label.clone(),
                completeness: doc.completeness,
                required: doc.required.clone(),
                missing: doc.missing.clone(),
                issues: doc.issues.clone(),
                fields: doc.fields.clone(),
                applied: Vec::new(),
                duplicate: !fresh,
                at: now_ms(),
                fingerprint: fp.clone(),
            },
            tool_image: None,
            mold_image: None,
            mold_geometry: None,
            wall: None,
            sds: Vec::new(),
            calibrations: Vec::new(),
            library: Vec::new(),
        };
        let image_ok = !doc.issues.iter().any(|i| i.level == "error");
        match doc.kind {
            DocKind::ToolImage if image_ok => match self.tool_image(models, sds, bytes, opt, fresh) {
                Ok(r) => {
                    out.doc.applied.push(format!("공구 이미지 형상 벡터 ({})", r.role));
                    out.sds.extend(r.sds.iter().cloned());
                    out.tool_image = Some(r);
                }
                Err(e) => out.doc.issues.push(Issue {
                    level: "error".into(),
                    field: "tool_image".into(),
                    message: e,
                }),
            },
            DocKind::MoldImage if image_ok => match self.mold_image(models, sds, bytes, opt, fresh) {
                Ok(r) => {
                    out.doc.applied.push(format!("금형 표면 이미지 벡터 ({})", r.role));
                    out.sds.extend(r.sds.iter().cloned());
                    out.mold_image = Some(r);
                }
                Err(e) => out.doc.issues.push(Issue {
                    level: "error".into(),
                    field: "mold_image".into(),
                    message: e,
                }),
            },
            DocKind::MoldDepth if image_ok => {
                self.set_ids(None, opt.mold_id.clone(), None);
                match (opt.mm_per_px.filter(|v| *v > 0.0), opt.depth_range_mm.filter(|v| *v > 0.0)) {
                    (Some(mpp), Some(range)) => {
                        let origin = (opt.origin_x.unwrap_or(0.0), opt.origin_y.unwrap_or(0.0));
                        match mold::heightmap_from_depth_image(bytes, mpp, range, opt.depth_invert.unwrap_or(false), origin)
                            .and_then(|h| self.mold_geometry(sds, h, opt.role.as_deref().unwrap_or("measured"), fresh))
                        {
                            Ok(r) => {
                                out.doc.applied.push(format!("금형 형상 벡터 ({}, mold.geom)", r.role));
                                out.sds.extend(r.sds.iter().cloned());
                                out.mold_geometry = Some(r);
                            }
                            Err(e) => out.doc.issues.push(Issue {
                                level: "error".into(),
                                field: "mold_depth".into(),
                                message: e,
                            }),
                        }
                    }
                    _ => out.doc.issues.push(Issue {
                        level: "error".into(),
                        field: "mold_depth".into(),
                        message: "깊이맵 이미지는 mm/px 와 깊이 범위(mm)를 함께 입력해야 합니다 (임의 가정 없이 중단)".into(),
                    }),
                }
            }
            DocKind::MoldCsv => {
                self.set_ids(None, opt.mold_id.clone(), None);
                let parsed = match opt.cell_mm.filter(|v| *v > 0.0) {
                    Some(c) => std::str::from_utf8(bytes)
                        .map_err(|e| e.to_string())
                        .and_then(|t| mold::heightmap_from_csv(t, Some(c))),
                    None => doc.heightmap.clone().ok_or_else(|| "높이맵을 해석하지 못했습니다".to_string()),
                };
                match parsed.and_then(|mut h| {
                    if let Some(x) = opt.origin_x {
                        h.x0 = x;
                    }
                    if let Some(y) = opt.origin_y {
                        h.y0 = y;
                    }
                    self.mold_geometry(sds, h, opt.role.as_deref().unwrap_or("measured"), fresh)
                }) {
                    Ok(r) => {
                        out.doc.applied.push(format!("금형 형상 벡터 ({}, mold.geom)", r.role));
                        out.sds.extend(r.sds.iter().cloned());
                        out.mold_geometry = Some(r);
                    }
                    Err(e) => out.doc.issues.push(Issue {
                        level: "error".into(),
                        field: "heightmap".into(),
                        message: e,
                    }),
                }
            }
            DocKind::WallCsv => {
                if let Some(pts) = doc.wall_points.clone().filter(|p| !p.is_empty()) {
                    match self.wall_cmm(sds, pts, fresh) {
                        Ok(r) => {
                            out.doc.applied.push(format!("벽면 CMM {}점 → 휨·마모·열 이득 분해", r.fit.points));
                            out.sds.extend(r.sds.iter().cloned());
                            out.wall = Some(r);
                        }
                        Err(e) => out.doc.issues.push(Issue {
                            level: "warn".into(),
                            field: "wall".into(),
                            message: e,
                        }),
                    }
                }
            }
            _ => {
                let (applied, log) = self.apply_document(&doc, sds, fresh, opt);
                out.doc.applied.extend(applied);
                out.sds.extend(log);
            }
        }
        if out.doc.issues.iter().any(|i| i.level == "error") {
            out.doc.completeness = (out.doc.completeness - 0.15).max(0.0);
        }
        let consumed = !out.doc.applied.is_empty();
        if consumed {
            self.mark_seen(&fp);
        }
        self.docs.retain(|d| d.name != out.doc.name || d.kind != out.doc.kind);
        self.docs.push(out.doc.clone());
        if self.docs.len() > 200 {
            let excess = self.docs.len() - 200;
            self.docs.drain(0..excess);
        }
        out.calibrations = std::mem::take(&mut self.pending_calib);
        Ok(out)
    }

    pub fn forecast(&mut self, models: &Models, sds: &SdsStore, key: &str, horizon: usize) -> Result<ForecastResult, String> {
        let s = self
            .series
            .get(key)
            .cloned()
            .ok_or_else(|| format!("'{}' 시계열이 없습니다. 측정 로그를 입력하거나 현재 공구 이미지를 누적 절삭 시간과 함께 입력하세요", key))?;
        if s.len() < 3 {
            return Err(format!("'{}' 측정점이 {}개라 예측할 수 없습니다 (최소 3개)", key, s.len()));
        }
        let (ep, n_eps) = timeseries::current_episode(&s);
        let analysis = self.ensure_analysis(sds);
        let (ctx, _) = self.context(sds);
        let part_min = self.sim.as_ref().map(|r| r.cut_time_min);
        let k = ep.minutes_per_t(part_min);
        let t0 = ep.t.first().cloned().unwrap_or(0.0);
        let y0 = ep.y.first().cloned().unwrap_or(0.0);
        let rate = match key {
            "vb_mm" => Some(analysis.wear.vb_rate_mm_per_min),
            "radial_loss_um" => Some(analysis.wear.radial_loss_rate_um_per_min),
            _ => None,
        };
        let scale = match key {
            "vb_mm" => Some(1.0),
            "radial_loss_um" => Some(ctx.tool.clearance_deg.to_radians().tan() * 1000.0),
            _ => None,
        };
        let traj = analysis.effective_trajectory();
        let life = traj.life_min;
        let prior: Option<Box<dyn Fn(f64) -> f64>> = match (scale, k) {
            (Some(sc), Some(k)) if !traj.t_min.is_empty() => {
                Some(Box::new(move |t: f64| y0 + traj.growth_from(y0 / sc, (t - t0).max(0.0) * k) * sc))
            }
            _ => None,
        };
        let threshold = self.threshold_for(key, &ctx);
        let mut f = timeseries::forecast(&ep, horizon.max(1), models.ttm.as_ref(), prior.as_deref(), threshold);
        f.episodes = n_eps;
        if n_eps > 1 {
            f.notes.push(format!("공구 교체로 보이는 리셋 {}회 → 마지막 구간 {}점만 사용", n_eps - 1, ep.len()));
        }
        if rate.is_some() && k.is_none() {
            f.notes.push("시간 단위를 알 수 없어(행 순번 또는 사이클 시간 미산출) 물리 사전식을 적용하지 않았습니다".into());
        }
        if prior.is_some() {
            f.notes.push(format!(
                "물리 사전식: 플랭크 마모–절삭력–온도 결합 3단계 마모 궤적(수명 {:.0}분, 초기 마모율 {:.5}/min)을 기준선으로 두고 잔차만 시계열 모델로 예측",
                life,
                rate.unwrap_or(0.0)
            ));
        }
        self.forecast = Some(f.clone());
        self.decision = None;
        Ok(f)
    }

    pub fn decide(&mut self, models: &Models, sds: &mut SdsStore, use_advisor: bool) -> Result<DecisionReport, String> {
        let analysis = self.ensure_analysis(sds);
        let (ctx, _) = self.context(sds);
        let finishing = self.purpose == Purpose::Finishing;
        let material_label = self.profile.workpiece_setup.material_label();
        let tool_label = format!(
            "{} diameter {:.1} mm, {} flutes, {} nose, coating {}",
            self.profile.endmill_setting.model,
            self.profile.endmill_setting.diameter_mm,
            self.profile.endmill_setting.flute_count,
            self.profile.endmill_setting.nose.key(),
            self.profile.endmill_setting.coating_name.clone().unwrap_or_else(|| "none".into())
        );
        let coolant_label = self.profile.coolant_config.method.key().to_string();
        let inp = DecisionInput {
            cut: Some(&analysis),
            sim: self.sim.as_ref(),
            wear: self.tool_wear.as_ref(),
            mold_surface: self.mold_wear.as_ref(),
            forecast: self.forecast.as_ref(),
            mold_fit: self.mold_fit.as_ref(),
            machine: &self.profile.machine,
            rpm: self.profile.conditions.spindle_rpm as f64,
            tolerance_mm: ctx.tolerance_mm,
            allowance_mm: ctx.allowance_mm,
            coating_max_temp_c: ctx.tool.coating.max_temp_c,
            vb_limit_mm: ctx.vb_limit_mm(),
            mc: ctx.wp.mc,
            workpiece_temp_limit_c: ctx.wp.temp_limit_c,
            material_label: material_label.clone(),
            tool_label,
            coolant_label,
            finishing,
        };
        let mut rep = decision::evaluate(&inp);
        if self.sim.is_none() {
            rep.notes.push("부하 시뮬레이션이 없어 경로 전체의 동력·휨·마모 누적은 판단에 포함되지 않았습니다".into());
        }
        if self.tool_wear.is_none() {
            rep.notes.push("공구 이미지 비교가 없어 실측 마모는 판단에 포함되지 않았습니다".into());
        }
        let scope = decision_scope(&self.profile);
        if use_advisor {
            match models.laya.as_ref() {
                Some(adv) => {
                    let allow = match sds.agreement_rate(&scope, "rule_vs_advisor") {
                        Some((r, n)) if n >= 10 && r < 0.5 => {
                            rep.notes.push(format!(
                                "이 스코프에서 규칙-조언 일치율 {:.0}% ({}건) 로 낮아 조언에 의한 상향을 막았습니다",
                                r * 100.0,
                                n
                            ));
                            false
                        }
                        _ => true,
                    };
                    decision::apply_advisor(&mut rep, adv, allow);
                    if let (Some(agreed), Some(aa)) = (rep.agreement, rep.advisor_action) {
                        sds.record_agreement(&scope, "rule_vs_advisor", agreed);
                        if !agreed {
                            let margin = rep.advisor.as_ref().map(|a| a.top_probability as f64).unwrap_or(0.0);
                            let other = if aa == rep.final_action { rep.rule_action } else { aa };
                            sds.record_confusion(&scope, rep.final_action.key(), other.key(), margin);
                        }
                    }
                }
                None => rep.advisor_error = Some("laya-typed-decisions 모델이 로드되지 않아 결정적 게이트만 사용했습니다".into()),
            }
        }
        let state = rep.final_action.key().to_string();
        if let Some(prev) = self.last_state.clone() {
            sds.record_transition(&scope, &prev, &state);
        }
        rep.transition_to_replace = sds.transition_prior(&scope, &state, Action::ReplaceTool.key());
        self.last_state = Some(state);
        self.decision = Some(rep.clone());
        Ok(rep)
    }

    pub fn apply_decision(&mut self) -> Result<Vec<String>, String> {
        let rep = self.decision.clone().ok_or_else(|| "먼저 '판단 실행' 을 하세요".to_string())?;
        let mut changes = Vec::new();
        let z = self.profile.endmill_setting.flute_count.max(1) as f64;
        if let Some(s) = rep.feed_scale.filter(|s| *s < 0.999) {
            let c = &mut self.profile.conditions;
            c.feed_per_tooth_mm *= s;
            c.feed_rate_mm_min = c.spindle_rpm as f64 * z * c.feed_per_tooth_mm;
            changes.push(format!("이송 ×{:.2} → fz {:.4} mm, F {:.0} mm/min", s, c.feed_per_tooth_mm, c.feed_rate_mm_min));
        }
        if let Some(s) = rep.speed_scale.filter(|s| *s < 0.999) {
            let d = self.profile.endmill_setting.diameter_mm;
            let c = &mut self.profile.conditions;
            let rpm = ((c.spindle_rpm as f64) * s).round().max(1.0) as u32;
            c.spindle_rpm = rpm;
            c.cutting_speed_m_min = std::f64::consts::PI * d * rpm as f64 / 1000.0;
            c.feed_rate_mm_min = rpm as f64 * z * c.feed_per_tooth_mm;
            changes.push(format!("회전수 ×{:.2} → S{} (Vc {:.0} m/min, fz 유지)", s, rpm, c.cutting_speed_m_min));
        }
        if let Some(key) = &rep.coolant_suggestion {
            if let Some(cfg) = CoolantConfig::from_key(key) {
                if cfg.method != self.profile.coolant_config.method {
                    changes.push(format!("냉각 {} → {}", self.profile.coolant_config.method.label(), cfg.method.label()));
                    self.profile.conditions.coolant = coolant_type_of(&cfg.method);
                    self.profile.coolant_config = cfg;
                }
            }
        }
        if changes.is_empty() {
            changes.push("반영할 절삭 조건 변경이 없습니다".into());
        } else {
            self.profile.modified_at = format!("{}", now_ms() / 1000);
            self.invalidate_process();
        }
        Ok(changes)
    }

    pub fn adaptive_program(&self) -> Result<AdaptiveProgram, String> {
        let sim = self
            .sim
            .as_ref()
            .ok_or_else(|| "먼저 '공정 해석'(부하 시뮬레이션)을 실행하세요".to_string())?;
        let (label, segs) = self.segments()?;
        let adapted = loadsim::adaptive_segments(&segs, &sim.episodes);
        let scaled = segs
            .iter()
            .zip(adapted.iter())
            .filter(|(a, b)| match (a.feed(), b.feed()) {
                (Some(x), Some(y)) => (x - y).abs() > 1e-9,
                _ => false,
            })
            .count();
        let min_scale = sim.episodes.iter().map(|e| e.feed_scale).fold(1.0, f64::min);
        let name = format!("{}_{}_ADAPT", self.profile.name.replace(' ', "_"), label);
        let prog = GCodeGenerator::program_from_segments(&self.profile, &adapted, &name);
        let base_meta = GCodeGenerator::metadata_for_segments(&self.profile, &segs, self.profile.machine.rapid_mm_min);
        let adapted_meta = GCodeGenerator::metadata_for_segments(&self.profile, &adapted, self.profile.machine.rapid_mm_min);
        Ok(AdaptiveProgram {
            program_name: name,
            text: prog.to_text(),
            scaled_segments: scaled,
            min_scale,
            time_base_min: base_meta.estimated_time_min,
            time_adapted_min: adapted_meta.estimated_time_min,
            metadata: adapted_meta,
        })
    }

    pub fn summary(&self) -> WorkspaceSummary {
        let p = &self.profile;
        let sig = p.endmill_setting.signature();
        let key = if self.tool_id.is_empty() { sig.clone() } else { self.tool_id.clone() };
        let dir = self.vectors_dir();
        let tool_reference: Vec<String> = ["side", "end"]
            .iter()
            .filter(|v| {
                dir.join(format!("{}.json", wear::sanitize(&format!("tool.image_{}_{}", key, v))))
                    .exists()
            })
            .map(|v| v.to_string())
            .collect();
        let mold_reference = dir
            .join(format!("{}.json", wear::sanitize(&format!("mold.image_{}_side", self.mold_id))))
            .exists();
        WorkspaceSummary {
            profile_name: p.name.clone(),
            tool: format!("{} (Ø{} {}날 {})", p.endmill_setting.model, p.endmill_setting.diameter_mm, p.endmill_setting.flute_count, p.endmill_setting.nose.label()),
            tool_signature: sig,
            tool_id: key,
            material: p.workpiece_setup.material_label(),
            material_scope: material_scope_key(&p.workpiece_setup.effective_material()),
            coolant: p.coolant_config.method.label(),
            purpose: purpose_key(self.purpose).into(),
            pattern_key: self.pattern_key.clone(),
            program: self.program.as_ref().map(|(n, s)| format!("{} ({}개 이동)", n, s.len())),
            mold_id: self.mold_id.clone(),
            cut_minutes: self.cut_minutes,
            rpm: p.conditions.spindle_rpm,
            feed_mm_min: p.conditions.feed_rate_mm_min,
            ap_mm: p.conditions.axial_doc_mm,
            ae_mm: p.conditions.radial_doc_mm,
            tolerance_mm: p.workpiece_setup.effective_tolerance_mm(),
            docs: self.docs.iter().rev().take(50).cloned().collect(),
            series: self
                .series
                .values()
                .map(|s| SeriesInfo {
                    key: s.key.clone(),
                    unit: s.unit.clone(),
                    t_unit: s.t_unit.clone(),
                    points: s.len(),
                    last: s.y.last().cloned(),
                    episodes: timeseries::episodes_of(s).len(),
                })
                .collect(),
            tool_reference,
            mold_reference,
            has_nominal_mold: self.nominal_mold.is_some(),
            has_measured_mold: self.measured_mold.is_some(),
            has_sim: self.sim.is_some(),
            has_forecast: self.forecast.is_some(),
            has_decision: self.decision.is_some(),
            calibration: self.calibration.clone(),
            frf: self.frf_setting(),
        }
    }
}

