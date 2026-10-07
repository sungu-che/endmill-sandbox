use serde::Serialize;
use endmill_model::*;

#[derive(Serialize)]
pub struct BrandResponse {
    pub name: String,
    pub country: String,
    pub primary_tier: String,
    pub signature_coating: Option<String>,
    pub specialties: Vec<String>,
}

#[derive(Serialize)]
pub struct ApplicationProfileResponse {
    pub industry: String,
    pub purpose: String,
    pub recommended_tier: String,
    pub workpiece: String,
    pub note: String,
}

#[derive(Serialize)]
pub struct CuttingConditionsResponse {
    pub cutting_speed_m_min: f64,
    pub feed_rate_mm_min: f64,
    pub feed_per_tooth_mm: f64,
    pub axial_doc_mm: f64,
    pub radial_doc_mm: f64,
    pub spindle_rpm: u32,
    pub coolant: String,
    pub mrr_cm3_min: f64,
    pub cutting_force_n: f64,
    pub spindle_power_kw: f64,
    pub tool_deflection_mm: f64,
    pub interface_temp_c: f64,
    pub tool_life_min: Option<f64>,
    pub wall_error_um: f64,
}

#[derive(Serialize)]
pub struct EndMillSpecResponse {
    pub model: String,
    pub brand: String,
    pub tier: String,
    pub diameter_mm: f64,
    pub length_of_cut_mm: f64,
    pub overall_length_mm: f64,
    pub shank_diameter_mm: f64,
    pub flute_geometry: String,
    pub helix_angle_deg: f64,
    pub variable_pitch: bool,
    pub coating_name: Option<String>,
    pub can_machine_hard_materials: bool,
    pub suitable_for_unmanned_op: bool,
    pub life_multiplier: f64,
}

fn country_to_string(country: &Country) -> String {
    match country {
        Country::SouthKorea => "한국".into(),
        Country::Japan => "일본".into(),
        Country::Germany => "독일".into(),
        Country::Sweden => "스웨덴".into(),
        Country::Switzerland => "스위스".into(),
        Country::China => "중국".into(),
        Country::Taiwan => "대만".into(),
        Country::Other(s) => s.clone(),
    }
}

fn tier_to_string(tier: &PriceTier) -> String {
    match tier {
        PriceTier::LowEnd => "저가형".into(),
        PriceTier::MidRange => "중가형".into(),
        PriceTier::HighEnd => "고급형".into(),
    }
}

fn workpiece_to_string(w: &WorkpieceMaterial) -> String {
    match w {
        WorkpieceMaterial::Aluminum => "알루미늄 합금".into(),
        WorkpieceMaterial::CarbonSteel => "탄소강".into(),
        WorkpieceMaterial::AlloySteel { hardness_hrc } => format!("합금강 (HRC {})", hardness_hrc),
        WorkpieceMaterial::StainlessSteel => "스테인리스강".into(),
        WorkpieceMaterial::Titanium => "티타늄 합금".into(),
        WorkpieceMaterial::Inconel => "인코넬".into(),
        WorkpieceMaterial::SuperAlloy => "수퍼알로이".into(),
        WorkpieceMaterial::CFRP => "CFRP".into(),
    }
}

#[tauri::command]
pub fn get_brands() -> Vec<BrandResponse> {
    known_brands()
        .into_iter()
        .map(|b| BrandResponse {
            name: b.name,
            country: country_to_string(&b.country),
            primary_tier: tier_to_string(&b.primary_tier),
            signature_coating: b.signature_coating,
            specialties: b.specialties,
        })
        .collect()
}

#[tauri::command]
pub fn get_application_profiles() -> Vec<ApplicationProfileResponse> {
    build_application_table()
        .into_iter()
        .map(|p| ApplicationProfileResponse {
            industry: format!("{:?}", p.industry),
            purpose: format!("{:?}", p.purpose),
            recommended_tier: tier_to_string(&p.recommended_tier),
            workpiece: workpiece_to_string(&p.workpiece),
            note: p.note,
        })
        .collect()
}

#[tauri::command]
pub fn calculate_cutting_conditions(
    workpiece: String,
    diameter_mm: f64,
    flute_count: u8,
    is_high_end: bool,
    hardness_hrc: Option<u8>,
) -> Result<CuttingConditionsResponse, String> {
    let wp = material_from_key(&workpiece, hardness_hrc)?;

    let conds = CuttingCalculator::recommend_conditions(&wp, diameter_mm, flute_count, is_high_end)
        .map_err(|e| e.to_string())?;

    let assumed_loc_mm = diameter_mm * 2.5;
    let physics_result = CuttingCalculator::simulate_physics(&wp, &conds, diameter_mm, assumed_loc_mm);

    Ok(CuttingConditionsResponse {
        cutting_speed_m_min: conds.cutting_speed_m_min,
        feed_rate_mm_min: conds.feed_rate_mm_min,
        feed_per_tooth_mm: conds.feed_per_tooth_mm,
        axial_doc_mm: conds.axial_doc_mm,
        radial_doc_mm: conds.radial_doc_mm,
        spindle_rpm: conds.spindle_rpm,
        coolant: format!("{:?}", conds.coolant),
        mrr_cm3_min: physics_result.mrr_cm3_min,
        cutting_force_n: physics_result.cutting_force_n,
        spindle_power_kw: physics_result.spindle_power_kw,
        tool_deflection_mm: physics_result.tool_deflection_mm,
        interface_temp_c: physics_result.interface_temp_c,
        tool_life_min: Some(physics_result.tool_life_min).filter(|v| v.is_finite()),
        wall_error_um: physics_result.wall_error_um,
    })
}

#[tauri::command]
pub fn get_sample_endmill() -> EndMillSpecResponse {
    let spec = sample_high_end_endmill();

    let can_hard = spec.can_machine_hard_materials();
    let suitable_unmanned = spec.suitable_for_unmanned_op();
    let life_mult = spec.estimated_life_multiplier_vs_low_end();

    EndMillSpecResponse {
        model: spec.model,
        brand: spec.brand.name,
        tier: tier_to_string(&spec.tier),
        diameter_mm: spec.diameter_mm,
        length_of_cut_mm: spec.length_of_cut_mm,
        overall_length_mm: spec.overall_length_mm,
        shank_diameter_mm: spec.shank_diameter_mm,
        flute_geometry: format!("{:?}", spec.flute_geometry),
        helix_angle_deg: spec.helix_angle_deg,
        variable_pitch: spec.variable_pitch,
        coating_name: spec.coating.as_ref().map(|c| c.name.clone()),
        can_machine_hard_materials: can_hard,
        suitable_for_unmanned_op: suitable_unmanned,
        life_multiplier: life_mult,
    }
}

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use endmill_model::gcode::{GCodeGenerator, ToolPathSegment};
use endmill_model::ingest::material_from_key;
use endmill_model::pipeline::{
    AdaptiveProgram, IngestOptions, IngestOutcome, ModelStatus, Models, ProcessOutcome, Workspace, WorkspaceSummary,
};
use endmill_model::profile::{
    CoolantConfig, EndMillMockupSetting, MachiningProfile, ProfileStore,
};
use endmill_model::sds::{SdsStatus, SdsStore};
use endmill_model::timeseries::ForecastResult;
use endmill_model::decision::DecisionReport;
use endmill_model::physics::Recommendation;
use endmill_model::workpiece_setup::{ClampingMethod, StockShape, WorkpieceSetup};
use endmill_model::cutting::WorkpieceMaterial;
use endmill_model::curation::{self, CurationReport, EndMillComparison, EndMillRelay};
use endmill_model::store::rdb::{EndMillRow, ProjectRow, RunRow, WorkpieceRow};
use endmill_model::store::series::{series_id, window_features};
use endmill_model::store::{Store, StoreStatus};
use endmill_model::calc::{CalcContext, CalcOutcome, CalcOverrides};
use endmill_model::modelhub::{DownloadJob, InstallStatus, ModelFile, ModelHub};
use endmill_model::viewport::{CoolantVisual, ViewportSim};

pub struct Shared {
    pub data_dir: PathBuf,
    pub store: Mutex<ProfileStore>,
    pub ws: Mutex<Workspace>,
    pub models: Mutex<Models>,
    pub sds: Mutex<SdsStore>,
    pub library: Mutex<Store>,
    pub library_fallback: bool,
    pub hub: Mutex<ModelHub>,
    pub settings: Mutex<AppSettings>,
    pub viewport_cache: Mutex<Option<(String, ViewportSim)>>,
    pub model_snapshot: Mutex<ModelStatus>,
    pub loading_model: Mutex<Option<String>>,
}

fn yes() -> bool {
    true
}

fn default_anim_speed() -> f64 {
    0.0
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct ModelSettings {
    #[serde(default = "yes")]
    pub auto_download: bool,
    #[serde(default)]
    pub startup_download: bool,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub token: String,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            auto_download: true,
            startup_download: false,
            endpoint: String::new(),
            token: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct UiSettings {
    #[serde(default = "default_anim_speed")]
    pub anim_speed: f64,
    #[serde(default = "yes")]
    pub show_tool: bool,
    #[serde(default = "yes")]
    pub show_coolant: bool,
    #[serde(default = "yes")]
    pub show_heat: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            anim_speed: default_anim_speed(),
            show_tool: true,
            show_coolant: true,
            show_heat: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize, Default)]
pub struct AppSettings {
    #[serde(default)]
    pub environment: ShopEnvironment,
    #[serde(default)]
    pub models: ModelSettings,
    #[serde(default)]
    pub ui: UiSettings,
}

fn load_settings(data_dir: &std::path::Path) -> AppSettings {
    fs::read_to_string(data_dir.join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<AppSettings>(&t).ok())
        .filter(|s| s.environment.validate().is_ok())
        .unwrap_or_default()
}

fn save_settings(sh: &Shared) -> Result<(), String> {
    let s = lock(&sh.settings).clone();
    let text = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?;
    fs::write(sh.data_dir.join("settings.json"), text).map_err(|e| format!("설정 저장 실패: {}", e))
}

pub struct AppState {
    pub shared: Arc<Shared>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    }
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&data_dir);
        let (mut library, library_fallback) = match Store::open(&data_dir.join("library")) {
            Ok(s) => (s, false),
            Err(e) => {
                let tmp = std::env::temp_dir().join("endmill-sandbox-library");
                let mut s = Store::open_with_memory_rdb(&tmp).expect("임시 라이브러리를 열 수 없습니다");
                s.notes.push(format!("라이브러리 SQLite 를 열 수 없어 임시 메모리 저장소로 동작합니다: {}", e));
                (s, true)
            }
        };
        if let Ok(log) = library.import_legacy(&data_dir) {
            library.notes.extend(log);
        }
        match library.ensure_builtin_presets(&MachiningPreset::builtin()) {
            Ok(added) if !added.is_empty() => library.notes.push(format!("기본 프리셋 {}개 추가: {}", added.len(), added.join(", "))),
            Err(e) => library.notes.push(format!("기본 프리셋 동기화 실패: {}", e)),
            _ => {}
        }
        let settings = load_settings(&data_dir);
        let mut store = match library.profile_store() {
            Ok(s) if !s.profiles.is_empty() => s,
            _ => {
                let presets = library.profile_store().map(|s| s.presets).unwrap_or_default();
                for preset in presets.iter().filter(|p| !p.name.trim().is_empty()) {
                    let p = MachiningProfile::from_preset(preset, &preset.name);
                    let _ = library.save_profile(&p, Some(&preset.name));
                }
                let mut seeded = library.profile_store().ok().filter(|s| !s.profiles.is_empty()).unwrap_or_else(build_demo_profile_store);
                let order: Vec<String> = MachiningPreset::builtin().into_iter().map(|p| p.name).collect();
                seeded.profiles.sort_by_key(|p| order.iter().position(|n| *n == p.name).unwrap_or(order.len()));
                seeded
            }
        };
        for p in store.profiles.iter_mut() {
            p.machine.environment = settings.environment.clone();
        }
        let first = store
            .profiles
            .first()
            .cloned()
            .unwrap_or_else(|| MachiningProfile::from_preset(&ProfileStore::new().presets[0], "기본"));
        let models_root = fs::read_to_string(data_dir.join("models_root.txt"))
            .map(|s| PathBuf::from(s.trim()))
            .unwrap_or_else(|_| data_dir.join("models"));
        let mut ws = Workspace::new(&data_dir, first);
        if let Ok(seen) = library.rdb.seen_all() {
            ws.extend_seen(seen);
        }
        let _ = library.restore_series(&mut ws);
        let models = Models::new(&models_root);
        let snapshot = models.status();
        let hub = ModelHub::new(&models_root, Some(&settings.models.endpoint), Some(&settings.models.token));
        if settings.models.startup_download {
            for spec in endmill_model::modelhub::catalog() {
                let st = endmill_model::modelhub::install_status(&models_root, &spec);
                if usable_model_dir(&models, &spec.key, &st).is_none() {
                    let _ = hub.start(&spec.key);
                }
            }
        }
        Self {
            shared: Arc::new(Shared {
                store: Mutex::new(store),
                ws: Mutex::new(ws),
                models: Mutex::new(models),
                sds: Mutex::new(SdsStore::open(&data_dir.join("sds"))),
                library: Mutex::new(library),
                library_fallback,
                hub: Mutex::new(hub),
                settings: Mutex::new(settings),
                viewport_cache: Mutex::new(None),
                model_snapshot: Mutex::new(snapshot),
                loading_model: Mutex::new(None),
                data_dir,
            }),
        }
    }

    pub fn flush(&self) {
        lock(&self.shared.sds).flush();
        persist_store(&self.shared);
        lock(&self.shared.library).flush();
    }
}

fn persist_store(sh: &Shared) {
    let store = lock(&sh.store);
    {
        let mut lib = lock(&sh.library);
        for p in store.profiles.iter() {
            let _ = lib.save_profile(p, None);
        }
    }
    if sh.library_fallback {
        let _ = store.save_to_file(&sh.data_dir.join("profiles.json").display().to_string());
    }
}

fn with_profile<T>(state: &tauri::State<'_, AppState>, name: &str, f: impl FnOnce(&MachiningProfile) -> Result<T, String>) -> Result<T, String> {
    let store = lock(&state.shared.store);
    let profile = store
        .get_by_name(name)
        .ok_or_else(|| format!("프로필을 찾을 수 없음: {}", name))?;
    f(profile)
}

fn sync_profile_to_store(sh: &Shared, profile: &MachiningProfile) {
    lock(&sh.store).upsert_profile(profile.clone());
    let _ = lock(&sh.library).save_profile(profile, None);
    if sh.library_fallback {
        persist_store(sh);
    }
}

async fn blocking<T, F>(state: &tauri::State<'_, AppState>, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&Shared) -> Result<T, String> + Send + 'static,
{
    let sh = state.shared.clone();
    tauri::async_runtime::spawn_blocking(move || f(&sh))
        .await
        .map_err(|e| format!("작업 실행 실패: {}", e))?
}

#[derive(Serialize)]
pub struct GCodePreviewResponse {
    pub program_name: String,
    pub line_count: usize,
    pub text: String,
    pub generated_at: String,
    pub workpiece_material: String,
    pub tool_model: String,
    pub coolant_method: String,
    pub estimated_time_min: f64,
    pub total_distance_mm: f64,
    pub warnings: Vec<String>,
    pub cutting_time_min: f64,
    pub rapid_time_min: f64,
}

#[derive(Serialize)]
pub struct EndMillSettingResponse {
    pub name: String,
    pub model: String,
    pub diameter_mm: f64,
    pub flute_count: u8,
    pub loc_mm: f64,
    pub oal_mm: f64,
    pub shank_diameter_mm: f64,
    pub helix_angle_deg: f64,
    pub coating_name: Option<String>,
    pub is_high_end: bool,
    pub nose: String,
    pub stickout_mm: f64,
}

#[derive(Serialize)]
pub struct PresetResponse {
    pub name: String,
    pub description: String,
    pub endmill: EndMillSettingResponse,
    pub workpiece: String,
    pub coolant: String,
}

#[derive(Serialize)]
pub struct ProfileResponse {
    pub name: String,
    pub description: String,
    pub endmill: EndMillSettingResponse,
    pub workpiece: String,
    pub coolant: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub modified_at: String,
}

#[derive(Serialize)]
pub struct WorkpieceSetupResponse {
    pub name: String,
    pub material: String,
    pub shape: String,
    pub width_mm: f64,
    pub height_mm: f64,
    pub thickness_mm: f64,
    pub clamping: String,
    pub stock_allowance_mm: f64,
    pub zero_point: (f64, f64),
    pub surface_roughness_target_ra: f64,
    pub hardness_hrc: Option<u8>,
    pub volume_cm3: f64,
    pub tolerance_mm: f64,
}

#[derive(Serialize)]
pub struct CoolantOptionResponse {
    pub key: String,
    pub label: String,
    pub m_code: String,
    pub pressure_bar: f64,
    pub flow_rate_l_min: f64,
    pub nozzle_count: u8,
}

#[derive(Serialize)]
pub struct ToolPathSegmentResponse {
    pub kind: String,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub z: Option<f64>,
    pub feed: Option<f64>,
    pub z_start: Option<f64>,
    pub i: Option<f64>,
    pub j: Option<f64>,
    pub rpm: u32,
}

fn endmill_to_response(e: &EndMillMockupSetting) -> EndMillSettingResponse {
    EndMillSettingResponse {
        name: e.name.clone(),
        model: e.model.clone(),
        diameter_mm: e.diameter_mm,
        flute_count: e.flute_count,
        loc_mm: e.loc_mm,
        oal_mm: e.oal_mm,
        shank_diameter_mm: e.shank_diameter_mm,
        helix_angle_deg: e.helix_angle_deg,
        coating_name: e.coating_name.clone(),
        is_high_end: e.is_high_end,
        nose: e.nose.label(),
        stickout_mm: e.effective_stickout_mm(),
    }
}

fn workpiece_to_response(w: &WorkpieceSetup) -> WorkpieceSetupResponse {
    WorkpieceSetupResponse {
        name: w.name.clone(),
        material: w.material_label(),
        shape: format!("{:?}", w.shape),
        width_mm: w.width_mm,
        height_mm: w.height_mm,
        thickness_mm: w.thickness_mm,
        clamping: format!("{:?}", w.clamping),
        stock_allowance_mm: w.stock_allowance_mm,
        zero_point: w.zero_point,
        surface_roughness_target_ra: w.surface_roughness_target_ra,
        hardness_hrc: w.hardness_hrc,
        volume_cm3: w.volume_cm3(),
        tolerance_mm: w.effective_tolerance_mm(),
    }
}

fn coolant_to_response(key: &str, cfg: &CoolantConfig) -> CoolantOptionResponse {
    CoolantOptionResponse {
        key: key.to_string(),
        label: cfg.method.label(),
        m_code: cfg.method.gcode_m_code().to_string(),
        pressure_bar: cfg.pressure_bar,
        flow_rate_l_min: cfg.flow_rate_l_min,
        nozzle_count: cfg.nozzle_count,
    }
}

fn profile_to_response(p: &MachiningProfile) -> ProfileResponse {
    ProfileResponse {
        name: p.name.clone(),
        description: p.description.clone(),
        endmill: endmill_to_response(&p.endmill_setting),
        workpiece: p.workpiece_setup.material_label(),
        coolant: p.coolant_config.method.label(),
        tags: p.tags.clone(),
        created_at: p.created_at.clone(),
        modified_at: p.modified_at.clone(),
    }
}

fn segments_to_response(segments: Vec<ToolPathSegment>, rpm: u32) -> Vec<ToolPathSegmentResponse> {
    segments
        .into_iter()
        .map(|s| {
            let kind = s.kind().to_string();
            let feed = s.feed();
            match s {
                ToolPathSegment::Linear { x, y, z, .. } | ToolPathSegment::Rapid { x, y, z } => ToolPathSegmentResponse {
                    kind, x: Some(x), y: Some(y), z: Some(z), feed, z_start: None, i: None, j: None, rpm,
                },
                ToolPathSegment::ArcCW { x, y, z, i, j, .. } | ToolPathSegment::ArcCCW { x, y, z, i, j, .. } => ToolPathSegmentResponse {
                    kind, x: Some(x), y: Some(y), z: Some(z), feed, z_start: None, i: Some(i), j: Some(j), rpm,
                },
                ToolPathSegment::Plunge { x, y, z_start, z_end, .. } => ToolPathSegmentResponse {
                    kind, x: Some(x), y: Some(y), z: Some(z_end), feed, z_start: Some(z_start), i: None, j: None, rpm,
                },
            }
        })
        .collect()
}

fn preview_response(program: &endmill_model::gcode::GCodeProgram, metadata: endmill_model::gcode::GCodeMetadata) -> GCodePreviewResponse {
    GCodePreviewResponse {
        program_name: program.program_name.clone(),
        line_count: program.line_count(),
        text: program.to_text(),
        generated_at: metadata.generated_at,
        workpiece_material: metadata.workpiece_material,
        tool_model: metadata.tool_model,
        coolant_method: metadata.coolant_method,
        estimated_time_min: metadata.estimated_time_min,
        total_distance_mm: metadata.total_distance_mm,
        warnings: metadata.warnings,
        cutting_time_min: metadata.cutting_time_min,
        rapid_time_min: metadata.rapid_time_min,
    }
}

fn build_demo_profile_store() -> ProfileStore {
    let mut store = ProfileStore::new();
    let presets = store.presets.clone();
    for preset in &presets {
        let profile = MachiningProfile::from_preset(preset, &preset.name);
        store.add_profile(profile);
    }
    store
}

#[tauri::command]
pub fn generate_gcode_preview(state: tauri::State<'_, AppState>, profile_name: String) -> Result<GCodePreviewResponse, String> {
    with_profile(&state, &profile_name, |profile| {
        let program = GCodeGenerator::generate_from_profile(profile);
        let metadata = GCodeGenerator::generate_metadata(profile);
        Ok(preview_response(&program, metadata))
    })
}

#[tauri::command]
pub fn save_gcode_to_file(
    state: tauri::State<'_, AppState>,
    profile_name: String,
    output_path: String,
) -> Result<String, String> {
    let text = with_profile(&state, &profile_name, |profile| Ok(GCodeGenerator::generate_from_profile(profile).to_text()))?;
    let path = PathBuf::from(&output_path);
    fs::write(&path, text).map_err(|e| format!("파일 쓰기 실패: {}", e))?;
    Ok(format!("저장 완료: {}", path.display()))
}

#[tauri::command]
pub fn list_mockup_settings() -> Vec<EndMillSettingResponse> {
    EndMillMockupSetting::builtin_tools().iter().map(endmill_to_response).collect()
}

#[tauri::command]
pub fn list_presets(state: tauri::State<'_, AppState>) -> Vec<PresetResponse> {
    let store = lock(&state.shared.store);
    store
        .presets
        .iter()
        .map(|p| PresetResponse {
            name: p.name.clone(),
            description: p.description.clone(),
            endmill: endmill_to_response(&p.endmill_setting),
            workpiece: p.workpiece_setup.material_label(),
            coolant: p.coolant_config.method.label(),
        })
        .collect()
}

#[tauri::command]
pub fn list_profiles(state: tauri::State<'_, AppState>) -> Vec<ProfileResponse> {
    let store = lock(&state.shared.store);
    store.profiles.iter().map(profile_to_response).collect()
}

#[tauri::command]
pub fn save_profile_to_json(
    state: tauri::State<'_, AppState>,
    profile_name: String,
    output_path: String,
) -> Result<String, String> {
    let json = with_profile(&state, &profile_name, |profile| {
        serde_json::to_string_pretty(profile).map_err(|e| format!("직렬화 실패: {}", e))
    })?;
    fs::write(&output_path, json).map_err(|e| format!("파일 쓰기 실패: {}", e))?;
    Ok(format!("프로필 저장 완료: {}", output_path))
}

#[tauri::command]
pub fn load_profile_from_json(state: tauri::State<'_, AppState>, input_path: String) -> Result<ProfileResponse, String> {
    let json = fs::read_to_string(&input_path)
        .map_err(|e| format!("파일 읽기 실패: {}", e))?;
    let mut profile: MachiningProfile =
        serde_json::from_str(&json).map_err(|e| format!("역직렬화 실패: {}", e))?;
    profile.endmill_setting.validate()?;
    profile.normalize_legacy();
    stamp_environment(&state.shared, &mut profile);
    sync_profile_to_store(&state.shared, &profile);
    Ok(profile_to_response(&profile))
}

#[tauri::command]
pub fn list_workpiece_setups() -> Vec<WorkpieceSetupResponse> {
    WorkpieceSetup::builtin_setups().iter().map(workpiece_to_response).collect()
}

#[tauri::command]
pub fn build_custom_workpiece(
    name: String,
    material_key: String,
    shape_key: String,
    width_mm: f64,
    height_mm: f64,
    thickness_mm: f64,
    clamping_key: String,
    stock_allowance_mm: f64,
    hardness_hrc: Option<u8>,
    tolerance_mm: Option<f64>,
) -> Result<WorkpieceSetupResponse, String> {
    let material = material_from_key(&material_key, hardness_hrc)?;
    let shape = match shape_key.as_str() {
        "rectangular" => StockShape::Rectangular,
        "cylindrical" => StockShape::Cylindrical { diameter_mm: width_mm },
        _ => StockShape::Custom,
    };
    let clamping = match clamping_key.as_str() {
        "vise" => ClampingMethod::Vise,
        "vacuum" => ClampingMethod::VacuumChuck,
        "magnetic" => ClampingMethod::MagneticChuck,
        "fixture" => ClampingMethod::FixturePlate,
        "soft_jaw" => ClampingMethod::SoftJaw,
        _ => return Err(format!("지원하지 않는 클램핑: {}", clamping_key)),
    };
    let setup = WorkpieceSetup {
        name,
        material,
        shape,
        width_mm,
        height_mm,
        thickness_mm,
        clamping,
        stock_allowance_mm,
        zero_point: (0.0, 0.0),
        surface_roughness_target_ra: 1.6,
        hardness_hrc,
        grain_direction_deg: 0.0,
        pre_machined: false,
        tolerance_mm: tolerance_mm.filter(|t| *t > 0.0),
    };
    Ok(workpiece_to_response(&setup))
}

#[tauri::command]
pub fn list_coolant_options() -> Vec<CoolantOptionResponse> {
    vec![
        coolant_to_response("air_blast", &CoolantConfig::air_blast()),
        coolant_to_response("flood", &CoolantConfig::flood()),
        coolant_to_response("mist", &CoolantConfig::mist()),
        coolant_to_response("through_tool", &CoolantConfig::through_tool()),
        coolant_to_response("dry", &CoolantConfig::dry()),
    ]
}

#[tauri::command]
pub async fn apply_coolant_to_profile(
    state: tauri::State<'_, AppState>,
    profile_name: String,
    coolant_key: String,
) -> Result<CoolantOptionResponse, String> {
    blocking(&state, move |sh| {
        let profile = set_coolant(sh, &profile_name, &coolant_key, None, None, None)?;
        Ok(coolant_to_response(&coolant_key, &profile.coolant_config))
    })
    .await
}

#[tauri::command]
pub fn get_toolpath_segments(state: tauri::State<'_, AppState>, profile_name: String) -> Result<Vec<ToolPathSegmentResponse>, String> {
    with_profile(&state, &profile_name, |profile| {
        Ok(segments_to_response(
            GCodeGenerator::generate_synthetic_segments(profile),
            profile.conditions.spindle_rpm,
        ))
    })
}

#[tauri::command]
pub fn create_custom_endmill(
    name: String,
    model: String,
    diameter_mm: f64,
    flute_count: u8,
    loc_mm: f64,
    oal_mm: f64,
    shank_diameter_mm: f64,
    helix_angle_deg: f64,
    coating_name: Option<String>,
    is_high_end: bool,
) -> Result<EndMillSettingResponse, String> {
    let setting = EndMillMockupSetting::custom(
        name, model, diameter_mm, flute_count,
        loc_mm, oal_mm, shank_diameter_mm,
        helix_angle_deg, coating_name, is_high_end,
    );
    setting.validate()?;
    Ok(endmill_to_response(&setting))
}

#[tauri::command]
pub fn create_custom_profile(
    state: tauri::State<'_, AppState>,
    name: String,
    description: String,
    endmill_name: String,
    endmill_model: String,
    diameter_mm: f64,
    flute_count: u8,
    loc_mm: f64,
    oal_mm: f64,
    shank_diameter_mm: f64,
    helix_angle_deg: f64,
    coating_name: Option<String>,
    is_high_end: bool,
    workpiece_material_key: String,
    workpiece_width: f64,
    workpiece_height: f64,
    workpiece_thickness: f64,
    coolant_key: String,
    tags: Vec<String>,
    hardness_hrc: Option<u8>,
    tolerance_mm: Option<f64>,
) -> Result<ProfileResponse, String> {
    let endmill = EndMillMockupSetting::custom(
        endmill_name, endmill_model, diameter_mm, flute_count,
        loc_mm, oal_mm, shank_diameter_mm, helix_angle_deg,
        coating_name, is_high_end,
    );
    endmill.validate()?;

    let material = material_from_key(&workpiece_material_key, hardness_hrc)?;

    let conditions = CuttingCalculator::recommend_conditions(
        &material, diameter_mm, flute_count, is_high_end,
    ).map_err(|e| e.to_string())?;

    let workpiece = WorkpieceSetup {
        name: format!("{} 가공물", workpiece_material_key),
        material,
        shape: StockShape::Rectangular,
        width_mm: workpiece_width,
        height_mm: workpiece_height,
        thickness_mm: workpiece_thickness,
        clamping: ClampingMethod::Vise,
        stock_allowance_mm: 0.5,
        zero_point: (0.0, 0.0),
        surface_roughness_target_ra: 1.6,
        hardness_hrc,
        grain_direction_deg: 0.0,
        pre_machined: false,
        tolerance_mm: tolerance_mm.filter(|t| *t > 0.0),
    };

    let coolant = CoolantConfig::from_key(&coolant_key)
        .ok_or_else(|| format!("지원하지 않는 냉각: {}", coolant_key))?;

    let mut profile = {
        let mut store = lock(&state.shared.store);
        store
            .create_profile_from_custom(name, description, endmill, conditions, workpiece, coolant, tags)?
            .clone()
    };
    stamp_environment(&state.shared, &mut profile);
    lock(&state.shared.store).upsert_profile(profile.clone());
    persist_store(&state.shared);

    Ok(profile_to_response(&profile))
}

#[tauri::command]
pub fn generate_gcode_with_pattern(
    state: tauri::State<'_, AppState>,
    profile_name: String,
    pattern_key: String,
) -> Result<GCodePreviewResponse, String> {
    with_profile(&state, &profile_name, |profile| {
        let pattern = ToolPathPattern::from_key(&pattern_key, profile)?;
        let program = GCodeGenerator::generate_gcode_with_pattern(profile, &pattern);
        let metadata = GCodeGenerator::generate_metadata_with_pattern(profile, &pattern);
        Ok(preview_response(&program, metadata))
    })
}

#[tauri::command]
pub fn get_toolpath_with_pattern(
    state: tauri::State<'_, AppState>,
    profile_name: String,
    pattern_key: String,
) -> Result<Vec<ToolPathSegmentResponse>, String> {
    with_profile(&state, &profile_name, |profile| {
        let pattern = ToolPathPattern::from_key(&pattern_key, profile)?;
        Ok(segments_to_response(
            GCodeGenerator::generate_synthetic_segments_with_pattern(profile, &pattern),
            profile.conditions.spindle_rpm,
        ))
    })
}

#[derive(Serialize)]
pub struct CoolantVisualResponse {
    pub key: String,
    pub label: String,
    pub m_code: String,
    pub pressure_bar: f64,
    pub flow_rate_l_min: f64,
    pub nozzle_count: u8,
    pub nozzle_angle_deg: f64,
    pub nozzle_distance_mm: f64,
    pub spray_type: String,
    pub color_hex: String,
}

#[tauri::command]
pub fn get_coolant_visual_info(coolant_key: String) -> Result<CoolantVisualResponse, String> {
    let (cfg, spray, color) = match coolant_key.as_str() {
        "air_blast" => (CoolantConfig::air_blast(), "gas", "#87CEEB"),
        "flood" => (CoolantConfig::flood(), "liquid_stream", "#4169E1"),
        "mist" => (CoolantConfig::mist(), "mist_particles", "#ADD8E6"),
        "through_tool" => (CoolantConfig::through_tool(), "internal_jet", "#00CED1"),
        "dry" => (CoolantConfig::dry(), "none", "#666666"),
        _ => return Err(format!("지원하지 않는 냉각: {}", coolant_key)),
    };
    Ok(CoolantVisualResponse {
        key: coolant_key,
        label: cfg.method.label(),
        m_code: cfg.method.gcode_m_code().to_string(),
        pressure_bar: cfg.pressure_bar,
        flow_rate_l_min: cfg.flow_rate_l_min,
        nozzle_count: cfg.nozzle_count,
        nozzle_angle_deg: cfg.nozzle_angle_deg,
        nozzle_distance_mm: cfg.nozzle_distance_mm,
        spray_type: spray.to_string(),
        color_hex: color.to_string(),
    })
}

#[tauri::command]
pub fn save_gcode_with_pattern(
    state: tauri::State<'_, AppState>,
    profile_name: String,
    pattern_key: String,
    output_path: String,
) -> Result<String, String> {
    let text = with_profile(&state, &profile_name, |profile| {
        let pattern = ToolPathPattern::from_key(&pattern_key, profile)?;
        Ok(GCodeGenerator::generate_gcode_with_pattern(profile, &pattern).to_text())
    })?;
    let path = PathBuf::from(&output_path);
    fs::write(&path, text).map_err(|e| format!("파일 쓰기 실패: {}", e))?;
    Ok(format!("저장 완료: {}", path.display()))
}

#[tauri::command]
pub fn save_text_file(output_path: String, text: String) -> Result<String, String> {
    let path = PathBuf::from(&output_path);
    fs::write(&path, text).map_err(|e| format!("파일 쓰기 실패: {}", e))?;
    Ok(format!("저장 완료: {}", path.display()))
}

#[tauri::command]
pub async fn ws_summary(state: tauri::State<'_, AppState>) -> Result<WorkspaceSummary, String> {
    blocking(&state, |sh| Ok(lock(&sh.ws).summary())).await
}

#[tauri::command]
pub async fn ws_select_profile(state: tauri::State<'_, AppState>, profile_name: String) -> Result<WorkspaceSummary, String> {
    blocking(&state, move |sh| {
        let profile = lock(&sh.store)
            .get_by_name(&profile_name)
            .cloned()
            .ok_or_else(|| format!("프로필을 찾을 수 없음: {}", profile_name))?;
        let mut ws = lock(&sh.ws);
        if ws.profile.name != profile.name
            || serde_json::to_string(&ws.profile).ok() != serde_json::to_string(&profile).ok()
        {
            ws.set_profile(profile);
            let _ = lock(&sh.library).restore_series(&mut ws);
        }
        Ok(ws.summary())
    })
    .await
}

#[tauri::command]
pub async fn ws_set_options(
    state: tauri::State<'_, AppState>,
    purpose: Option<String>,
    pattern_key: Option<String>,
    tool_id: Option<String>,
    mold_id: Option<String>,
    cut_minutes: Option<f64>,
    clear_program: Option<bool>,
) -> Result<WorkspaceSummary, String> {
    blocking(&state, move |sh| {
        let mut ws = lock(&sh.ws);
        let key_before = ws.tool_key();
        if let Some(p) = purpose {
            ws.set_purpose(&p);
        }
        if clear_program.unwrap_or(false) {
            ws.clear_program();
        }
        if let Some(k) = pattern_key {
            if ws.pattern_key != k {
                ws.set_pattern(&k)?;
            }
        }
        ws.set_ids(tool_id, mold_id, cut_minutes);
        if ws.tool_key() != key_before {
            let _ = lock(&sh.library).restore_series(&mut ws);
        }
        Ok(ws.summary())
    })
    .await
}

#[tauri::command]
pub async fn ws_ingest(
    state: tauri::State<'_, AppState>,
    name: String,
    data_b64: String,
    options: Option<IngestOptions>,
) -> Result<IngestOutcome, String> {
    blocking(&state, move |sh| {
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data_b64.trim())
            .map_err(|e| format!("파일 데이터 디코딩 실패: {}", e))?;
        let opt = options.unwrap_or_default();
        let (out, profile) = {
            let mut ws = lock(&sh.ws);
            if let Some(t) = opt.tool_id.as_ref() {
                let next = if t.trim().is_empty() { ws.profile.endmill_setting.signature() } else { t.trim().to_string() };
                if next != ws.tool_key() {
                    ws.set_ids(Some(t.clone()), None, None);
                    let _ = lock(&sh.library).restore_series(&mut ws);
                }
            }
            let key_before = ws.tool_key();
            let models = lock(&sh.models);
            let mut sds = lock(&sh.sds);
            let mut out = ws.ingest(&models, &mut sds, &name, &bytes, &opt)?;
            drop(sds);
            drop(models);
            let mut lib = lock(&sh.library);
            if ws.tool_key() != key_before {
                let _ = lib.restore_series(&mut ws);
            }
            match lib.record_ingest(&ws, &out) {
                Ok(log) => out.library = log,
                Err(e) => out.library.push(format!("라이브러리 기록 실패: {}", e)),
            }
            (out, ws.profile.clone())
        };
        if !out.doc.applied.is_empty() {
            let env = lock(&sh.settings).environment.clone();
            let mut profile = profile;
            if profile.machine.environment != env {
                profile.machine.environment = env.clone();
                lock(&sh.ws).set_environment(&env);
            }
            sync_profile_to_store(sh, &profile);
        }
        Ok(out)
    })
    .await
}

#[tauri::command]
pub async fn ws_set_frf(
    state: tauri::State<'_, AppState>,
    fn_hz: Option<f64>,
    k_n_per_um: Option<f64>,
    zeta: Option<f64>,
    clear: Option<bool>,
) -> Result<WorkspaceSummary, String> {
    blocking(&state, move |sh| {
        let (summary, profile) = {
            let mut ws = lock(&sh.ws);
            ws.set_frf(fn_hz, k_n_per_um, zeta, clear.unwrap_or(false))?;
            (ws.summary(), ws.profile.clone())
        };
        sync_profile_to_store(sh, &profile);
        Ok(summary)
    })
    .await
}

#[tauri::command]
pub async fn ws_run_process(state: tauri::State<'_, AppState>) -> Result<ProcessOutcome, String> {
    blocking(&state, |sh| {
        let mut ws = lock(&sh.ws);
        let sds = lock(&sh.sds);
        let mut out = ws.run_process(&sds)?;
        drop(sds);
        match lock(&sh.library).record_process(&ws, &out) {
            Ok(log) => out.library = log,
            Err(e) => out.library.push(format!("라이브러리 기록 실패: {}", e)),
        }
        Ok(out)
    })
    .await
}

#[tauri::command]
pub async fn ws_recommend(state: tauri::State<'_, AppState>, apply: bool) -> Result<Recommendation, String> {
    blocking(&state, move |sh| {
        let (rec, profile) = {
            let mut ws = lock(&sh.ws);
            let sds = lock(&sh.sds);
            let rec = ws.recommend(&sds, apply)?;
            (rec, ws.profile.clone())
        };
        if apply {
            sync_profile_to_store(sh, &profile);
        }
        Ok(rec)
    })
    .await
}

#[tauri::command]
pub async fn ws_forecast(state: tauri::State<'_, AppState>, key: String, horizon: Option<usize>) -> Result<ForecastResult, String> {
    blocking(&state, move |sh| {
        let mut ws = lock(&sh.ws);
        let models = lock(&sh.models);
        let sds = lock(&sh.sds);
        ws.forecast(&models, &sds, &key, horizon.unwrap_or(16))
    })
    .await
}

#[tauri::command]
pub async fn ws_decide(state: tauri::State<'_, AppState>, use_advisor: bool) -> Result<DecisionReport, String> {
    blocking(&state, move |sh| {
        let mut ws = lock(&sh.ws);
        let models = lock(&sh.models);
        let mut sds = lock(&sh.sds);
        let mut rep = ws.decide(&models, &mut sds, use_advisor)?;
        drop(sds);
        drop(models);
        match lock(&sh.library).record_decision(&ws, &rep) {
            Ok(log) => rep.notes.extend(log),
            Err(e) => rep.notes.push(format!("라이브러리 기록 실패: {}", e)),
        }
        Ok(rep)
    })
    .await
}

#[tauri::command]
pub async fn ws_apply_decision(state: tauri::State<'_, AppState>) -> Result<Vec<String>, String> {
    blocking(&state, |sh| {
        let (changes, profile) = {
            let mut ws = lock(&sh.ws);
            let changes = ws.apply_decision()?;
            (changes, ws.profile.clone())
        };
        sync_profile_to_store(sh, &profile);
        Ok(changes)
    })
    .await
}

#[tauri::command]
pub async fn ws_adaptive_program(state: tauri::State<'_, AppState>) -> Result<AdaptiveProgram, String> {
    blocking(&state, |sh| lock(&sh.ws).adaptive_program()).await
}

#[tauri::command]
pub async fn models_status(state: tauri::State<'_, AppState>) -> Result<ModelStatus, String> {
    blocking(&state, |sh| Ok(lock(&sh.models).status())).await
}

#[tauri::command]
pub async fn models_set_root(state: tauri::State<'_, AppState>, root: String) -> Result<ModelStatus, String> {
    blocking(&state, move |sh| {
        let dir = PathBuf::from(root.trim());
        if !dir.is_dir() {
            return Err(format!("폴더가 없습니다: {}", dir.display()));
        }
        lock(&sh.hub).set_root(&dir)?;
        let _ = fs::write(sh.data_dir.join("models_root.txt"), dir.display().to_string());
        let mut models = lock(&sh.models);
        models.set_root(&dir);
        let st = models.status();
        *lock(&sh.model_snapshot) = st.clone();
        Ok(st)
    })
    .await
}

#[tauri::command]
pub async fn models_load(state: tauri::State<'_, AppState>, which: String) -> Result<ModelStatus, String> {
    blocking(&state, move |sh| {
        let mut models = lock(&sh.models);
        let keys: Vec<&str> = match which.as_str() {
            "all" => vec!["siglip", "ttm", "laya"],
            "siglip" => vec!["siglip"],
            "ttm" => vec!["ttm"],
            "laya" => vec!["laya"],
            other => return Err(format!("알 수 없는 모델: {}", other)),
        };
        for k in keys {
            *lock(&sh.loading_model) = Some(k.to_string());
            let _ = models.load(k);
        }
        *lock(&sh.loading_model) = None;
        let st = models.status();
        *lock(&sh.model_snapshot) = st.clone();
        Ok(st)
    })
    .await
}

#[tauri::command]
pub async fn sds_status(state: tauri::State<'_, AppState>) -> Result<SdsStatus, String> {
    blocking(&state, |sh| Ok(lock(&sh.sds).status(80))).await
}

#[tauri::command]
pub async fn sds_flush(state: tauri::State<'_, AppState>) -> Result<Vec<String>, String> {
    blocking(&state, |sh| Ok(lock(&sh.sds).flush())).await
}

#[tauri::command]
pub async fn sds_purge(state: tauri::State<'_, AppState>) -> Result<SdsStatus, String> {
    blocking(&state, |sh| {
        let mut sds = lock(&sh.sds);
        sds.purge();
        Ok(sds.status(80))
    })
    .await
}

#[derive(Serialize)]
pub struct PresetBrief {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub builtin: bool,
    pub endmill_id: i64,
    pub workpiece_id: i64,
    pub profiles: i64,
    pub runs: i64,
}

#[derive(Serialize)]
pub struct ProfileBrief {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub preset_id: Option<i64>,
    pub endmill_id: i64,
    pub workpiece_id: i64,
    pub runs: i64,
    pub updated_at: i64,
}

#[derive(Serialize)]
pub struct LibraryOverview {
    pub status: StoreStatus,
    pub active_project: i64,
    pub projects: Vec<ProjectRow>,
    pub presets: Vec<PresetBrief>,
    pub profiles: Vec<ProfileBrief>,
    pub endmills: Vec<EndMillRow>,
    pub workpieces: Vec<WorkpieceRow>,
}

fn profile_brief(r: endmill_model::store::rdb::ProfileRow) -> ProfileBrief {
    ProfileBrief {
        id: r.id,
        project_id: r.project_id,
        name: r.name,
        preset_id: r.preset_id,
        endmill_id: r.endmill_id,
        workpiece_id: r.workpiece_id,
        runs: r.runs,
        updated_at: r.updated_at,
    }
}

fn overview(sh: &Shared) -> Result<LibraryOverview, String> {
    let lib = lock(&sh.library);
    Ok(LibraryOverview {
        status: lib.status(),
        active_project: lib.project_id,
        projects: lib.rdb.list_projects()?,
        presets: lib
            .rdb
            .list_presets()?
            .into_iter()
            .map(|p| PresetBrief {
                id: p.id,
                name: p.name,
                description: p.description,
                builtin: p.builtin,
                endmill_id: p.endmill_id,
                workpiece_id: p.workpiece_id,
                profiles: p.profiles,
                runs: p.runs,
            })
            .collect(),
        profiles: lib.rdb.list_profiles(lib.project_id)?.into_iter().map(profile_brief).collect(),
        endmills: lib.rdb.list_endmills(500)?,
        workpieces: lib.rdb.list_workpieces(500)?,
    })
}

fn switch_project(sh: &Shared, id: i64) -> Result<(), String> {
    let ps = {
        let mut ws = lock(&sh.ws);
        let mut lib = lock(&sh.library);
        lib.use_project(id)?;
        let mut ps = lib.profile_store()?;
        if ps.profiles.is_empty() {
            lib.save_profile(&ws.profile, None)?;
            ps = lib.profile_store()?;
        }
        let pick = ps
            .profiles
            .iter()
            .find(|p| p.name == ws.profile.name)
            .or_else(|| ps.profiles.first())
            .cloned();
        if let Some(p) = pick {
            ws.set_profile(p);
        }
        let _ = lib.restore_series(&mut ws);
        ps
    };
    *lock(&sh.store) = ps;
    Ok(())
}

fn curve_query(ws: &Workspace, project_id: i64) -> Option<(String, Vec<f32>, String)> {
    let part = ws.sim.as_ref().map(|s| s.cut_time_min);
    for key in ["vb_mm", "radial_loss_um"] {
        if let Some(s) = ws.series.get(key) {
            let (ep, _) = endmill_model::timeseries::current_episode(s);
            let thr = ws.threshold(key)?;
            if let Some((v, _, _, _)) = window_features(&ep.t, &ep.y, thr, ep.minutes_per_t(part)) {
                return Some((key.to_string(), v, series_id(project_id, &ws.tool_key(), key)));
            }
        }
    }
    None
}

fn scope_of(lib: &Store, all_projects: Option<bool>, default_all: bool) -> Option<i64> {
    if all_projects.unwrap_or(default_all) {
        None
    } else {
        Some(lib.project_id)
    }
}

#[tauri::command]
pub async fn lib_overview(state: tauri::State<'_, AppState>) -> Result<LibraryOverview, String> {
    blocking(&state, overview).await
}

#[tauri::command]
pub async fn lib_create_project(state: tauri::State<'_, AppState>, name: String, description: Option<String>) -> Result<LibraryOverview, String> {
    blocking(&state, move |sh| {
        let id = lock(&sh.library).rdb.ensure_project(&name, description.as_deref().unwrap_or(""))?;
        switch_project(sh, id)?;
        overview(sh)
    })
    .await
}

#[tauri::command]
pub async fn lib_use_project(state: tauri::State<'_, AppState>, project_id: i64) -> Result<LibraryOverview, String> {
    blocking(&state, move |sh| {
        switch_project(sh, project_id)?;
        overview(sh)
    })
    .await
}

#[tauri::command]
pub async fn lib_save_profile(state: tauri::State<'_, AppState>, preset_name: Option<String>) -> Result<LibraryOverview, String> {
    blocking(&state, move |sh| {
        let profile = lock(&sh.ws).profile.clone();
        lock(&sh.store).upsert_profile(profile.clone());
        lock(&sh.library).save_profile(&profile, preset_name.as_deref().map(str::trim).filter(|s| !s.is_empty()))?;
        overview(sh)
    })
    .await
}

#[tauri::command]
pub async fn lib_save_preset(state: tauri::State<'_, AppState>, name: String, description: Option<String>) -> Result<LibraryOverview, String> {
    blocking(&state, move |sh| {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err("프리셋 이름을 입력하세요".into());
        }
        let (p, purpose) = {
            let ws = lock(&sh.ws);
            (ws.profile.clone(), endmill_model::pipeline::purpose_key(ws.purpose).to_string())
        };
        let preset = MachiningPreset {
            name: name.clone(),
            description: description.filter(|d| !d.trim().is_empty()).unwrap_or_else(|| p.description.clone()),
            endmill_setting: p.endmill_setting.clone(),
            conditions: p.conditions.clone(),
            workpiece_setup: p.workpiece_setup.clone(),
            coolant_config: p.coolant_config.clone(),
            purpose: Some(purpose),
        };
        {
            let mut lib = lock(&sh.library);
            lib.save_preset(&preset)?;
            lib.save_profile(&p, Some(&name))?;
        }
        {
            let mut store = lock(&sh.store);
            store.presets.retain(|x| x.name != preset.name);
            store.presets.push(preset);
        }
        overview(sh)
    })
    .await
}

#[tauri::command]
pub async fn lib_profiles_by_preset(state: tauri::State<'_, AppState>, preset_id: i64) -> Result<Vec<ProfileBrief>, String> {
    blocking(&state, move |sh| {
        Ok(lock(&sh.library).rdb.profiles_by_preset(preset_id)?.into_iter().map(profile_brief).collect())
    })
    .await
}

#[tauri::command]
pub async fn lib_runs(
    state: tauri::State<'_, AppState>,
    endmill_id: Option<i64>,
    workpiece_id: Option<i64>,
    all_projects: Option<bool>,
    limit: Option<usize>,
) -> Result<Vec<RunRow>, String> {
    blocking(&state, move |sh| {
        let lib = lock(&sh.library);
        lib.rdb.runs(scope_of(&lib, all_projects, false), endmill_id, workpiece_id, limit.unwrap_or(100))
    })
    .await
}

#[tauri::command]
pub async fn lib_curate(state: tauri::State<'_, AppState>, all_projects: Option<bool>) -> Result<CurationReport, String> {
    blocking(&state, move |sh| {
        let ws = lock(&sh.ws);
        let mut lib = lock(&sh.library);
        let scope = scope_of(&lib, all_projects, true);
        let curve = curve_query(&ws, lib.project_id);
        let curve_ref = curve.as_ref().map(|(m, v, sid)| (m.as_str(), v.as_slice(), Some(sid.as_str())));
        curation::curate_for_workpiece(
            &mut lib,
            &ws.profile.workpiece_setup,
            Some(&ws.profile.endmill_setting),
            &ws.profile.machine,
            &ws.profile.coolant_config,
            ws.purpose,
            scope,
            curve_ref,
            10,
        )
    })
    .await
}

#[tauri::command]
pub async fn lib_compare_endmills(state: tauri::State<'_, AppState>, a_id: i64, b_id: i64, all_projects: Option<bool>) -> Result<EndMillComparison, String> {
    blocking(&state, move |sh| {
        let ws = lock(&sh.ws);
        let mut lib = lock(&sh.library);
        let scope = scope_of(&lib, all_projects, true);
        curation::compare_endmills(
            &mut lib,
            a_id,
            b_id,
            &ws.profile.workpiece_setup,
            &ws.profile.machine,
            &ws.profile.coolant_config,
            ws.purpose,
            scope,
        )
    })
    .await
}

#[tauri::command]
pub async fn lib_relay_endmill(state: tauri::State<'_, AppState>, endmill_id: i64, all_projects: Option<bool>) -> Result<EndMillRelay, String> {
    blocking(&state, move |sh| {
        let mut lib = lock(&sh.library);
        let scope = scope_of(&lib, all_projects, true);
        curation::relay_for_endmill(&mut lib, endmill_id, scope)
    })
    .await
}

#[tauri::command]
pub async fn lib_apply_endmill(state: tauri::State<'_, AppState>, endmill_id: i64) -> Result<WorkspaceSummary, String> {
    blocking(&state, move |sh| {
        let row = lock(&sh.library)
            .rdb
            .endmill(endmill_id)?
            .ok_or_else(|| format!("앤드밀 #{} 이(가) 라이브러리에 없습니다", endmill_id))?;
        let (summary, profile) = {
            let mut ws = lock(&sh.ws);
            let sds = lock(&sh.sds);
            let mut setting = row.setting();
            setting.stickout_mm = ws.profile.endmill_setting.stickout_mm;
            setting.runout_um = ws.profile.endmill_setting.runout_um;
            if setting.validate().is_err() {
                setting.stickout_mm = None;
            }
            setting.validate()?;
            let mut p = ws.profile.clone();
            p.endmill_setting = setting;
            ws.set_profile(p);
            ws.recommend(&sds, true)?;
            drop(sds);
            let _ = lock(&sh.library).restore_series(&mut ws);
            (ws.summary(), ws.profile.clone())
        };
        sync_profile_to_store(sh, &profile);
        Ok(summary)
    })
    .await
}

#[tauri::command]
pub async fn lib_flush(state: tauri::State<'_, AppState>) -> Result<Vec<String>, String> {
    blocking(&state, |sh| Ok(lock(&sh.library).flush())).await
}

#[tauri::command]
pub async fn lib_reindex(state: tauri::State<'_, AppState>) -> Result<String, String> {
    blocking(&state, |sh| {
        let n = lock(&sh.library).reindex_attributes()?;
        Ok(format!("SQLite 기준 속성 벡터 {}건을 LanceDB 에 다시 색인했습니다", n))
    })
    .await
}

fn stamp_environment(sh: &Shared, p: &mut MachiningProfile) {
    p.machine.environment = lock(&sh.settings).environment.clone();
}

fn now_secs() -> String {
    format!("{}", endmill_model::store::now_ms() / 1000)
}

fn profile_by_name(sh: &Shared, name: Option<&str>) -> Result<MachiningProfile, String> {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => {
            if let Some(p) = lock(&sh.store).get_by_name(n).cloned() {
                return Ok(p);
            }
            let ws = lock(&sh.ws);
            if ws.profile.name == n {
                return Ok(ws.profile.clone());
            }
            Err(format!("프로필을 찾을 수 없음: {}", n))
        }
        None => Ok(lock(&sh.ws).profile.clone()),
    }
}

fn commit_profile(sh: &Shared, profile: &MachiningProfile) {
    sync_profile_to_store(sh, profile);
    let mut ws = lock(&sh.ws);
    if ws.profile.name == profile.name {
        ws.set_profile(profile.clone());
        let _ = lock(&sh.library).restore_series(&mut ws);
    }
}

#[derive(Serialize)]
pub struct ProfileDetail {
    pub name: String,
    pub description: String,
    pub tool: EndMillMockupSetting,
    pub tool_label: String,
    pub stickout_mm: f64,
    pub workpiece: WorkpieceSetupResponse,
    pub coolant: CoolantVisual,
    pub conditions: endmill_model::cutting::CuttingConditions,
    pub environment: ShopEnvironment,
    pub context: CalcContext,
    pub modified_at: String,
}

fn detail_of(p: &MachiningProfile) -> ProfileDetail {
    ProfileDetail {
        name: p.name.clone(),
        description: p.description.clone(),
        tool: p.endmill_setting.clone(),
        tool_label: format!("{} · {}", p.endmill_setting.name, p.endmill_setting.shape_label()),
        stickout_mm: p.endmill_setting.effective_stickout_mm(),
        workpiece: workpiece_to_response(&p.workpiece_setup),
        coolant: CoolantVisual::of(&p.coolant_config, &p.machine.environment),
        conditions: p.conditions.clone(),
        environment: p.machine.environment.clone(),
        context: endmill_model::calc::context_of(p, None),
        modified_at: p.modified_at.clone(),
    }
}

fn recommend_into(sh: &Shared, p: &MachiningProfile) -> Result<MachiningProfile, String> {
    let sds = lock(&sh.sds);
    let (out, _) = endmill_model::calc::apply(p, &CalcOverrides::default(), Some(&sds))?;
    Ok(out)
}

fn set_coolant(sh: &Shared, name: &str, key: &str, temperature_c: Option<f64>, pressure_bar: Option<f64>, flow_rate_l_min: Option<f64>) -> Result<MachiningProfile, String> {
    let mut cfg = CoolantConfig::from_key(key).ok_or_else(|| format!("지원하지 않는 냉각: {}", key))?;
    if let Some(t) = temperature_c {
        if !(t.is_finite() && (0.0..=60.0).contains(&t)) {
            return Err("냉각 매체 온도는 0~60 °C 범위로 입력하세요".into());
        }
        cfg.temperature_c = Some(t);
    }
    if let Some(pb) = pressure_bar {
        if !(pb.is_finite() && (0.0..=150.0).contains(&pb)) {
            return Err("냉각 압력은 0~150 bar 범위로 입력하세요".into());
        }
        cfg.pressure_bar = pb;
    }
    if let Some(f) = flow_rate_l_min {
        if !(f.is_finite() && (0.0..=200.0).contains(&f)) {
            return Err("냉각 유량은 0~200 L/min 범위로 입력하세요".into());
        }
        cfg.flow_rate_l_min = f;
    }
    let mut p = profile_by_name(sh, Some(name))?;
    p.conditions.coolant = endmill_model::pipeline::coolant_type_of(&cfg.method);
    p.coolant_config = cfg;
    p.modified_at = now_secs();
    commit_profile(sh, &p);
    Ok(p)
}

#[derive(Serialize)]
pub struct ToolEntry {
    pub key: String,
    pub source: String,
    pub endmill_id: Option<i64>,
    pub label: String,
    pub attr_key: String,
    pub setting: EndMillMockupSetting,
    pub shape: String,
    pub substrate: String,
    pub coating_family: String,
    pub runs: i64,
    pub refs: i64,
    pub deletable: bool,
}

fn tool_catalog_of(sh: &Shared) -> Result<Vec<ToolEntry>, String> {
    use endmill_model::store::attr::EndMillAttr;
    let lib = lock(&sh.library);
    let rows = lib.rdb.list_endmills(500)?;
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for s in EndMillMockupSetting::builtin_tools() {
        let attr = EndMillAttr::from_setting(&s);
        let key = attr.key();
        let row = rows.iter().find(|r| r.attr_key == key);
        out.push(ToolEntry {
            key: format!("builtin:{}", s.model),
            source: "builtin".into(),
            endmill_id: row.map(|r| r.id),
            label: s.name.clone(),
            attr_key: key.clone(),
            shape: s.shape_label(),
            substrate: s.substrate.label().into(),
            coating_family: attr.coating_family.clone(),
            runs: row.map(|r| r.runs).unwrap_or(0),
            refs: 0,
            deletable: false,
            setting: s,
        });
        seen.insert(key);
    }
    for r in rows.iter() {
        if seen.contains(&r.attr_key) {
            continue;
        }
        let (a, b, c) = lib.rdb.endmill_refs(r.id)?;
        let setting = r.setting();
        out.push(ToolEntry {
            key: format!("lib:{}", r.id),
            source: "library".into(),
            endmill_id: Some(r.id),
            label: r.primary_alias().map(|a| a.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| r.label.clone()),
            attr_key: r.attr_key.clone(),
            shape: setting.shape_label(),
            substrate: setting.substrate.label().into(),
            coating_family: r.attr.coating_family.clone(),
            runs: r.runs,
            refs: a + b + c,
            deletable: a + b + c == 0,
            setting,
        });
    }
    Ok(out)
}

fn resolve_tool(sh: &Shared, key: &str) -> Result<EndMillMockupSetting, String> {
    if let Some(model) = key.strip_prefix("builtin:") {
        return EndMillMockupSetting::builtin_tools()
            .into_iter()
            .find(|t| t.model == model)
            .ok_or_else(|| format!("기본 공구를 찾을 수 없습니다: {}", model));
    }
    if let Some(id) = key.strip_prefix("lib:").and_then(|v| v.parse::<i64>().ok()) {
        return lock(&sh.library)
            .rdb
            .endmill(id)?
            .map(|r| r.setting())
            .ok_or_else(|| format!("앤드밀 #{} 이(가) 라이브러리에 없습니다", id));
    }
    Err(format!("알 수 없는 공구 키: {}", key))
}

#[derive(Serialize)]
pub struct WorkpieceEntry {
    pub key: String,
    pub source: String,
    pub workpiece_id: Option<i64>,
    pub name: String,
    pub label: String,
    pub material_key: String,
    pub setup: WorkpieceSetupResponse,
    pub refs: i64,
    pub deletable: bool,
}

fn workpiece_catalog_of(sh: &Shared) -> Result<Vec<WorkpieceEntry>, String> {
    use endmill_model::store::attr::WorkpieceAttr;
    let lib = lock(&sh.library);
    let rows = lib.rdb.list_workpieces(500)?;
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for w in WorkpieceSetup::builtin_setups() {
        let key = WorkpieceAttr::from_setup(&w).key();
        let row = rows.iter().find(|r| r.attr_key == key);
        out.push(WorkpieceEntry {
            key: format!("builtin:{}", w.name),
            source: "builtin".into(),
            workpiece_id: row.map(|r| r.id),
            name: w.name.clone(),
            label: w.material_label(),
            material_key: w.effective_material().family_key().into(),
            setup: workpiece_to_response(&w),
            refs: 0,
            deletable: false,
        });
        seen.insert(key);
    }
    for r in rows.iter() {
        if seen.contains(&r.attr_key) {
            continue;
        }
        let (a, b, c) = lib.rdb.workpiece_refs(r.id)?;
        out.push(WorkpieceEntry {
            key: format!("lib:{}", r.id),
            source: "library".into(),
            workpiece_id: Some(r.id),
            name: r.names.first().cloned().unwrap_or_else(|| r.setup.name.clone()),
            label: r.label.clone(),
            material_key: r.setup.effective_material().family_key().into(),
            setup: workpiece_to_response(&r.setup),
            refs: a + b + c,
            deletable: a + b + c == 0,
        });
    }
    Ok(out)
}

fn resolve_workpiece(sh: &Shared, key: &str) -> Result<WorkpieceSetup, String> {
    if let Some(name) = key.strip_prefix("builtin:") {
        return WorkpieceSetup::builtin_setups()
            .into_iter()
            .find(|w| w.name == name)
            .ok_or_else(|| format!("기본 원소재를 찾을 수 없습니다: {}", name));
    }
    if let Some(id) = key.strip_prefix("lib:").and_then(|v| v.parse::<i64>().ok()) {
        return lock(&sh.library)
            .rdb
            .workpiece(id)?
            .map(|r| r.setup)
            .ok_or_else(|| format!("공작물 #{} 이(가) 라이브러리에 없습니다", id));
    }
    Err(format!("알 수 없는 원소재 키: {}", key))
}

fn apply_tool(sh: &Shared, profile_name: &str, setting: EndMillMockupSetting, recommend: bool) -> Result<MachiningProfile, String> {
    let mut p = profile_by_name(sh, Some(profile_name))?;
    let mut s = setting;
    s.stickout_mm = p.endmill_setting.stickout_mm;
    s.runout_um = s.runout_um.or(p.endmill_setting.runout_um);
    if s.validate().is_err() {
        s.stickout_mm = None;
    }
    s.validate()?;
    p.endmill_setting = s;
    if recommend {
        p = recommend_into(sh, &p)?;
    }
    p.modified_at = now_secs();
    commit_profile(sh, &p);
    Ok(p)
}

fn apply_workpiece(sh: &Shared, profile_name: &str, setup: WorkpieceSetup, recommend: bool) -> Result<MachiningProfile, String> {
    let mut p = profile_by_name(sh, Some(profile_name))?;
    p.workpiece_setup = setup;
    if recommend {
        p = recommend_into(sh, &p)?;
    }
    p.modified_at = now_secs();
    commit_profile(sh, &p);
    Ok(p)
}

#[tauri::command]
pub async fn profile_detail(state: tauri::State<'_, AppState>, profile_name: Option<String>) -> Result<ProfileDetail, String> {
    blocking(&state, move |sh| Ok(detail_of(&profile_by_name(sh, profile_name.as_deref())?))).await
}

#[tauri::command]
pub async fn profile_set_tool(state: tauri::State<'_, AppState>, profile_name: String, tool_key: String, recommend: Option<bool>) -> Result<ProfileDetail, String> {
    blocking(&state, move |sh| {
        let setting = resolve_tool(sh, &tool_key)?;
        Ok(detail_of(&apply_tool(sh, &profile_name, setting, recommend.unwrap_or(true))?))
    })
    .await
}

#[tauri::command]
pub async fn profile_set_workpiece(state: tauri::State<'_, AppState>, profile_name: String, workpiece_key: String, recommend: Option<bool>) -> Result<ProfileDetail, String> {
    blocking(&state, move |sh| {
        let setup = resolve_workpiece(sh, &workpiece_key)?;
        Ok(detail_of(&apply_workpiece(sh, &profile_name, setup, recommend.unwrap_or(true))?))
    })
    .await
}

#[tauri::command]
pub async fn profile_set_coolant(
    state: tauri::State<'_, AppState>,
    profile_name: String,
    coolant_key: String,
    temperature_c: Option<f64>,
    pressure_bar: Option<f64>,
    flow_rate_l_min: Option<f64>,
) -> Result<ProfileDetail, String> {
    blocking(&state, move |sh| Ok(detail_of(&set_coolant(sh, &profile_name, &coolant_key, temperature_c, pressure_bar, flow_rate_l_min)?))).await
}

#[tauri::command]
pub async fn calc_context(state: tauri::State<'_, AppState>, profile_name: Option<String>, purpose: Option<String>) -> Result<CalcContext, String> {
    blocking(&state, move |sh| {
        let p = profile_by_name(sh, profile_name.as_deref())?;
        Ok(endmill_model::calc::context_of(&p, purpose.as_deref()))
    })
    .await
}

#[tauri::command]
pub async fn calc_run(state: tauri::State<'_, AppState>, profile_name: Option<String>, overrides: Option<CalcOverrides>) -> Result<CalcOutcome, String> {
    blocking(&state, move |sh| {
        let p = profile_by_name(sh, profile_name.as_deref())?;
        let sds = lock(&sh.sds);
        endmill_model::calc::run(&p, &overrides.unwrap_or_default(), Some(&sds)).map(|(_, o)| o)
    })
    .await
}

#[derive(Serialize)]
pub struct CalcApplyResponse {
    pub outcome: CalcOutcome,
    pub profile: ProfileDetail,
}

#[tauri::command]
pub async fn calc_apply(state: tauri::State<'_, AppState>, profile_name: Option<String>, overrides: Option<CalcOverrides>) -> Result<CalcApplyResponse, String> {
    blocking(&state, move |sh| {
        let p = profile_by_name(sh, profile_name.as_deref())?;
        let (applied, outcome) = {
            let sds = lock(&sh.sds);
            endmill_model::calc::apply(&p, &overrides.unwrap_or_default(), Some(&sds))?
        };
        commit_profile(sh, &applied);
        Ok(CalcApplyResponse {
            profile: detail_of(&applied),
            outcome,
        })
    })
    .await
}

#[tauri::command]
pub async fn tool_catalog(state: tauri::State<'_, AppState>) -> Result<Vec<ToolEntry>, String> {
    blocking(&state, tool_catalog_of).await
}

#[derive(Serialize)]
pub struct ToolPreview {
    pub valid: bool,
    pub error: Option<String>,
    pub label: String,
    pub attr_key: String,
    pub stickout_mm: f64,
    pub stiffness_n_per_um: f64,
    pub fn_hz: f64,
    pub deflection_um_per_100n: f64,
    pub mass_g: f64,
    pub coating_family: String,
    pub coating_max_c: f64,
    pub substrate_label: String,
    pub substrate_max_c: f64,
    pub suited: Vec<String>,
    pub cautions: Vec<String>,
}

fn preview_of(s: &EndMillMockupSetting) -> ToolPreview {
    use endmill_model::cutting::WorkpieceMaterial as M;
    use endmill_model::physics::{tool_compliance, CutContext, ToolGeometry, WorkpieceProps};
    use endmill_model::store::attr::EndMillAttr;
    let attr = EndMillAttr::from_setting(s);
    let mut out = ToolPreview {
        valid: true,
        error: None,
        label: attr.label(),
        attr_key: attr.key(),
        stickout_mm: s.effective_stickout_mm(),
        stiffness_n_per_um: 0.0,
        fn_hz: 0.0,
        deflection_um_per_100n: 0.0,
        mass_g: 0.0,
        coating_family: attr.coating_family.clone(),
        coating_max_c: endmill_model::physics::tribology_from_name(s.coating_name.as_deref()).max_temp_c,
        substrate_label: s.substrate.label().into(),
        substrate_max_c: s.substrate.max_temp_c(),
        suited: Vec::new(),
        cautions: Vec::new(),
    };
    if let Err(e) = s.validate() {
        out.valid = false;
        out.error = Some(e);
        return out;
    }
    let steel = WorkpieceProps::of(&M::CarbonSteel);
    let machine = MachineLimits::default();
    let tg = ToolGeometry::from_setting(s, &steel, &machine, Some(&s.substrate.base()));
    let ap = (0.5 * s.diameter_mm).min(s.loc_mm);
    let comp = tool_compliance(&tg, ap, 1.0);
    let modal = endmill_model::dynamics::estimated_modal(&tg, ap, 1.0);
    out.stiffness_n_per_um = comp.stiffness_n_per_um;
    out.fn_hz = modal.fn_hz;
    out.deflection_um_per_100n = 100.0 / comp.stiffness_n_per_um.max(1e-9);
    let d = s.diameter_mm;
    let (lf, ln, dn) = match s.neck() {
        Some((dn, reach)) => (s.loc_mm, reach - s.loc_mm, dn),
        None => (s.loc_mm, 0.0, 0.0),
    };
    let ls = (s.oal_mm - lf - ln).max(0.0);
    let vol = 0.65 * std::f64::consts::PI * d * d / 4.0 * lf + std::f64::consts::PI * dn * dn / 4.0 * ln + std::f64::consts::PI * s.shank_diameter_mm.powi(2) / 4.0 * ls;
    out.mass_g = tg.tool_density * vol * 1e-9 * 1000.0;
    let materials = [
        (M::Aluminum, "알루미늄"),
        (M::CarbonSteel, "탄소강"),
        (M::StainlessSteel, "스테인리스"),
        (M::AlloySteel { hardness_hrc: 55 }, "고경도강 HRC55"),
        (M::Titanium, "티타늄"),
        (M::Inconel, "인코넬"),
        (M::CFRP, "CFRP"),
    ];
    for (m, label) in materials.iter() {
        let mut prof = MachiningProfile::from_preset(&MachiningPreset::default_steel_general(), "preview");
        prof.endmill_setting = s.clone();
        prof.workpiece_setup.material = m.clone();
        if let M::AlloySteel { hardness_hrc } = m {
            prof.workpiece_setup.hardness_hrc = Some(*hardness_hrc);
        }
        prof.coolant_config = endmill_model::physics::WorkpieceProps::of(m).reference_coolant();
        let ctx = CutContext::from_profile(&prof);
        let t = endmill_model::tribology::reference_interface_c(&ctx.wp);
        let compat = endmill_model::tribology::compatibility(&ctx, t);
        let hard = ctx.wp.hardness_hrc >= 45.0 || matches!(m, M::Inconel | M::Titanium);
        let substrate_bad = match s.substrate {
            ToolSubstrate::Hss | ToolSubstrate::HssCobalt => hard,
            ToolSubstrate::Pcd => matches!(ctx.wp.chem, endmill_model::tribology::WpChem::Iron | endmill_model::tribology::WpChem::Nickel | endmill_model::tribology::WpChem::Titanium),
            ToolSubstrate::Cbn => matches!(m, M::Aluminum | M::CFRP),
            _ => false,
        };
        if compat.severity >= 2 || substrate_bad {
            let reason = if substrate_bad {
                format!("{} 모재 부적합", s.substrate.label())
            } else {
                compat.messages.first().cloned().unwrap_or_default()
            };
            out.cautions.push(format!("{}: {}", label, reason));
        } else {
            out.suited.push(label.to_string());
        }
    }
    if out.fn_hz > 0.0 && s.effective_stickout_mm() / d > 5.0 {
        out.cautions.push(format!("돌출/직경 {:.1} — 채터에 민감하니 탭 테스트로 고유진동수 확인 권장", s.effective_stickout_mm() / d));
    }
    out
}

#[tauri::command]
pub fn tool_preview(setting: EndMillMockupSetting) -> ToolPreview {
    preview_of(&setting)
}

#[derive(Serialize)]
pub struct ToolSaveResponse {
    pub key: String,
    pub endmill_id: i64,
    pub catalog: Vec<ToolEntry>,
    pub profile: Option<ProfileDetail>,
}

#[tauri::command]
pub async fn tool_save(state: tauri::State<'_, AppState>, setting: EndMillMockupSetting, apply_to: Option<String>) -> Result<ToolSaveResponse, String> {
    blocking(&state, move |sh| {
        let mut s = setting;
        s.name = s.name.trim().to_string();
        s.model = s.model.trim().to_string();
        if s.name.is_empty() {
            return Err("공구 이름을 입력하세요".into());
        }
        if s.model.is_empty() {
            s.model = format!("CUSTOM-{}", endmill_model::store::now_ms() % 1_000_000);
        }
        s.validate()?;
        let id = lock(&sh.library).index_endmill(&s)?;
        let profile = match apply_to.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) => Some(detail_of(&apply_tool(sh, name, s.clone(), true)?)),
            None => None,
        };
        let catalog = tool_catalog_of(sh)?;
        let key = catalog
            .iter()
            .find(|e| e.endmill_id == Some(id))
            .map(|e| e.key.clone())
            .unwrap_or_else(|| format!("lib:{}", id));
        Ok(ToolSaveResponse { key, endmill_id: id, catalog, profile })
    })
    .await
}

#[tauri::command]
pub async fn tool_delete(state: tauri::State<'_, AppState>, endmill_id: i64) -> Result<Vec<ToolEntry>, String> {
    blocking(&state, move |sh| {
        if !lock(&sh.library).delete_endmill(endmill_id)? {
            return Err(format!("앤드밀 #{} 이(가) 없습니다", endmill_id));
        }
        tool_catalog_of(sh)
    })
    .await
}

#[tauri::command]
pub async fn workpiece_catalog(state: tauri::State<'_, AppState>) -> Result<Vec<WorkpieceEntry>, String> {
    blocking(&state, workpiece_catalog_of).await
}

#[derive(Serialize)]
pub struct WorkpieceSaveResponse {
    pub key: String,
    pub workpiece_id: i64,
    pub setup: WorkpieceSetupResponse,
    pub catalog: Vec<WorkpieceEntry>,
    pub profile: Option<ProfileDetail>,
}

#[tauri::command]
pub async fn workpiece_save(
    state: tauri::State<'_, AppState>,
    name: String,
    material_key: String,
    shape_key: String,
    width_mm: f64,
    height_mm: f64,
    thickness_mm: f64,
    clamping_key: String,
    stock_allowance_mm: f64,
    hardness_hrc: Option<u8>,
    tolerance_mm: Option<f64>,
    surface_roughness_ra: Option<f64>,
    apply_to: Option<String>,
) -> Result<WorkpieceSaveResponse, String> {
    blocking(&state, move |sh| {
        let name = name.trim().to_string();
        if name.is_empty() {
            return Err("원소재 이름을 입력하세요".into());
        }
        if !(width_mm > 0.0 && height_mm > 0.0 && thickness_mm > 0.0) {
            return Err("폭·높이·두께는 0보다 커야 합니다".into());
        }
        if stock_allowance_mm < 0.0 {
            return Err("여유량은 0 이상이어야 합니다".into());
        }
        let material = material_from_key(&material_key, hardness_hrc)?;
        let shape = match shape_key.as_str() {
            "rectangular" => StockShape::Rectangular,
            "cylindrical" => StockShape::Cylindrical { diameter_mm: width_mm },
            _ => StockShape::Custom,
        };
        let clamping = match clamping_key.as_str() {
            "vise" => ClampingMethod::Vise,
            "vacuum" => ClampingMethod::VacuumChuck,
            "magnetic" => ClampingMethod::MagneticChuck,
            "fixture" => ClampingMethod::FixturePlate,
            "soft_jaw" => ClampingMethod::SoftJaw,
            other => return Err(format!("지원하지 않는 클램핑: {}", other)),
        };
        let setup = WorkpieceSetup {
            name,
            material,
            shape,
            width_mm,
            height_mm,
            thickness_mm,
            clamping,
            stock_allowance_mm,
            zero_point: (0.0, 0.0),
            surface_roughness_target_ra: surface_roughness_ra.filter(|r| *r > 0.0).unwrap_or(1.6),
            hardness_hrc,
            grain_direction_deg: 0.0,
            pre_machined: false,
            tolerance_mm: tolerance_mm.filter(|t| *t > 0.0),
        };
        let id = lock(&sh.library).index_workpiece(&setup)?;
        let profile = match apply_to.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => Some(detail_of(&apply_workpiece(sh, n, setup.clone(), true)?)),
            None => None,
        };
        let catalog = workpiece_catalog_of(sh)?;
        let key = catalog
            .iter()
            .find(|e| e.workpiece_id == Some(id))
            .map(|e| e.key.clone())
            .unwrap_or_else(|| format!("lib:{}", id));
        Ok(WorkpieceSaveResponse {
            key,
            workpiece_id: id,
            setup: workpiece_to_response(&setup),
            catalog,
            profile,
        })
    })
    .await
}

#[tauri::command]
pub async fn workpiece_delete(state: tauri::State<'_, AppState>, workpiece_id: i64) -> Result<Vec<WorkpieceEntry>, String> {
    blocking(&state, move |sh| {
        if !lock(&sh.library).delete_workpiece(workpiece_id)? {
            return Err(format!("공작물 #{} 이(가) 없습니다", workpiece_id));
        }
        workpiece_catalog_of(sh)
    })
    .await
}

#[derive(Serialize)]
pub struct PresetEntry {
    pub id: Option<i64>,
    pub name: String,
    pub description: String,
    pub builtin: bool,
    pub purpose: String,
    pub tool: String,
    pub workpiece: String,
    pub coolant: String,
    pub conditions: endmill_model::cutting::CuttingConditions,
    pub profiles: i64,
    pub runs: i64,
}

fn preset_entry(p: &MachiningPreset, id: Option<i64>, builtin: bool, profiles: i64, runs: i64) -> PresetEntry {
    PresetEntry {
        id,
        name: p.name.clone(),
        description: p.description.clone(),
        builtin,
        purpose: p.purpose.clone().unwrap_or_else(|| "roughing".into()),
        tool: format!("{} · {}", p.endmill_setting.name, p.endmill_setting.shape_label()),
        workpiece: p.workpiece_setup.material_label(),
        coolant: p.coolant_config.method.label(),
        conditions: p.conditions.clone(),
        profiles,
        runs,
    }
}

fn preset_catalog_of(sh: &Shared) -> Result<Vec<PresetEntry>, String> {
    let rows = lock(&sh.library).rdb.list_presets()?;
    if rows.is_empty() {
        return Ok(lock(&sh.store).presets.iter().map(|p| preset_entry(p, None, false, 0, 0)).collect());
    }
    Ok(rows.iter().map(|r| preset_entry(&r.preset, Some(r.id), r.builtin, r.profiles, r.runs)).collect())
}

#[tauri::command]
pub async fn preset_catalog(state: tauri::State<'_, AppState>) -> Result<Vec<PresetEntry>, String> {
    blocking(&state, preset_catalog_of).await
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct PresetCreateRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub tool_key: String,
    pub workpiece_key: String,
    pub coolant_key: String,
    #[serde(default)]
    pub purpose: Option<String>,
    #[serde(default = "yes")]
    pub create_profile: bool,
    #[serde(default)]
    pub profile_name: Option<String>,
    #[serde(default)]
    pub preview: bool,
}

#[derive(Serialize)]
pub struct PresetCreateResponse {
    pub saved: bool,
    pub preset: PresetEntry,
    pub profile_name: Option<String>,
    pub outcome: CalcOutcome,
}

fn unique_profile_name(sh: &Shared, base: &str) -> String {
    let store = lock(&sh.store);
    if store.get_by_name(base).is_none() {
        return base.to_string();
    }
    (2..1000)
        .map(|i| format!("{} ({})", base, i))
        .find(|n| store.get_by_name(n).is_none())
        .unwrap_or_else(|| format!("{} {}", base, endmill_model::store::now_ms()))
}

#[tauri::command]
pub async fn preset_create(state: tauri::State<'_, AppState>, request: PresetCreateRequest) -> Result<PresetCreateResponse, String> {
    blocking(&state, move |sh| {
        let req = request;
        let name = req.name.trim().to_string();
        if name.is_empty() {
            return Err("프리셋 이름을 입력하세요".into());
        }
        if !req.preview && lock(&sh.library).rdb.preset_id(&name)?.is_some() {
            return Err(format!("같은 이름의 프리셋이 이미 있습니다: {}", name));
        }
        let tool = resolve_tool(sh, &req.tool_key)?;
        let setup = resolve_workpiece(sh, &req.workpiece_key)?;
        let coolant = CoolantConfig::from_key(&req.coolant_key).ok_or_else(|| format!("지원하지 않는 냉각: {}", req.coolant_key))?;
        let purpose = req.purpose.clone().filter(|p| !p.trim().is_empty()).unwrap_or_else(|| "roughing".into());
        let mut base = lock(&sh.ws).profile.clone();
        base.name = name.clone();
        base.description = req.description.clone();
        base.endmill_setting = tool;
        base.endmill_setting.stickout_mm = None;
        base.workpiece_setup = setup;
        base.coolant_config = coolant;
        base.conditions.coolant = endmill_model::pipeline::coolant_type_of(&base.coolant_config.method);
        stamp_environment(sh, &mut base);
        let (tuned, outcome) = {
            let sds = lock(&sh.sds);
            endmill_model::calc::apply(&base, &CalcOverrides { purpose: Some(purpose.clone()), ..Default::default() }, Some(&sds))?
        };
        let preset = MachiningPreset {
            name: name.clone(),
            description: if req.description.trim().is_empty() {
                format!("{} · {} · {}", tuned.workpiece_setup.material_label(), tuned.endmill_setting.name, tuned.coolant_config.method.label())
            } else {
                req.description.trim().to_string()
            },
            endmill_setting: tuned.endmill_setting.clone(),
            conditions: tuned.conditions.clone(),
            workpiece_setup: tuned.workpiece_setup.clone(),
            coolant_config: tuned.coolant_config.clone(),
            purpose: Some(purpose),
        };
        if req.preview {
            return Ok(PresetCreateResponse {
                saved: false,
                preset: preset_entry(&preset, None, false, 0, 0),
                profile_name: None,
                outcome,
            });
        }
        let id = lock(&sh.library).save_preset(&preset)?;
        {
            let mut store = lock(&sh.store);
            store.presets.retain(|x| x.name != preset.name);
            store.presets.push(preset.clone());
        }
        let mut profile_name = None;
        if req.create_profile {
            let pname = unique_profile_name(sh, req.profile_name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&name));
            let mut prof = MachiningProfile::from_preset(&preset, &pname);
            prof.machine = tuned.machine.clone();
            lock(&sh.library).save_profile(&prof, Some(&preset.name))?;
            lock(&sh.store).upsert_profile(prof);
            profile_name = Some(pname);
        }
        Ok(PresetCreateResponse {
            saved: true,
            preset: preset_entry(&preset, Some(id), false, i64::from(profile_name.is_some()), 0),
            profile_name,
            outcome,
        })
    })
    .await
}

#[tauri::command]
pub async fn preset_delete(state: tauri::State<'_, AppState>, name: String) -> Result<Vec<PresetEntry>, String> {
    blocking(&state, move |sh| {
        let builtin = lock(&sh.library).rdb.list_presets()?.iter().any(|r| r.name == name && r.builtin);
        if builtin {
            return Err("기본 프리셋은 삭제할 수 없습니다 (복제한 프로필을 수정해 새 프리셋으로 저장하세요)".into());
        }
        if !lock(&sh.library).rdb.delete_preset(&name)? {
            return Err(format!("프리셋을 찾을 수 없습니다: {}", name));
        }
        lock(&sh.store).remove_preset_by_name(&name);
        preset_catalog_of(sh)
    })
    .await
}

#[tauri::command]
pub async fn profile_from_preset(state: tauri::State<'_, AppState>, preset_name: String, profile_name: Option<String>) -> Result<ProfileResponse, String> {
    blocking(&state, move |sh| {
        let preset = lock(&sh.store)
            .get_preset_by_name(&preset_name)
            .cloned()
            .or_else(|| lock(&sh.library).rdb.list_presets().ok()?.into_iter().find(|r| r.name == preset_name).map(|r| r.preset))
            .ok_or_else(|| format!("프리셋을 찾을 수 없습니다: {}", preset_name))?;
        let pname = unique_profile_name(sh, profile_name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&preset.name));
        let mut prof = MachiningProfile::from_preset(&preset, &pname);
        prof.machine = lock(&sh.ws).profile.machine.clone();
        stamp_environment(sh, &mut prof);
        lock(&sh.library).save_profile(&prof, Some(&preset.name))?;
        lock(&sh.store).upsert_profile(prof.clone());
        Ok(profile_to_response(&prof))
    })
    .await
}

#[tauri::command]
pub async fn profile_duplicate(state: tauri::State<'_, AppState>, source_name: String, new_name: Option<String>) -> Result<ProfileResponse, String> {
    blocking(&state, move |sh| {
        let mut p = profile_by_name(sh, Some(&source_name))?;
        let base = new_name.as_deref().map(str::trim).filter(|n| !n.is_empty()).map(|n| n.to_string()).unwrap_or_else(|| format!("{} 복사본", source_name));
        p.name = unique_profile_name(sh, &base);
        p.is_default = false;
        p.created_at = now_secs();
        p.modified_at = p.created_at.clone();
        lock(&sh.library).save_profile(&p, None)?;
        lock(&sh.store).upsert_profile(p.clone());
        Ok(profile_to_response(&p))
    })
    .await
}

#[tauri::command]
pub async fn profile_delete(state: tauri::State<'_, AppState>, name: String) -> Result<Vec<ProfileResponse>, String> {
    blocking(&state, move |sh| {
        if lock(&sh.store).profiles.len() <= 1 {
            return Err("마지막 프로필은 삭제할 수 없습니다".into());
        }
        {
            let lib = lock(&sh.library);
            lib.rdb.delete_profile(lib.project_id, &name)?;
        }
        let next = {
            let mut store = lock(&sh.store);
            if !store.remove_by_name(&name) {
                return Err(format!("프로필을 찾을 수 없음: {}", name));
            }
            store.profiles.first().cloned()
        };
        {
            let mut ws = lock(&sh.ws);
            if ws.profile.name == name {
                if let Some(p) = next {
                    ws.set_profile(p);
                    let _ = lock(&sh.library).restore_series(&mut ws);
                }
            }
        }
        Ok(lock(&sh.store).profiles.iter().map(profile_to_response).collect())
    })
    .await
}

#[tauri::command]
pub async fn viewport_sim(state: tauri::State<'_, AppState>, profile_name: Option<String>, pattern_key: String) -> Result<ViewportSim, String> {
    blocking(&state, move |sh| {
        let p = profile_by_name(sh, profile_name.as_deref())?;
        let key = format!("{}|{}", serde_json::to_string(&p).unwrap_or_default(), pattern_key);
        if let Some((k, v)) = lock(&sh.viewport_cache).as_ref() {
            if *k == key {
                return Ok(v.clone());
            }
        }
        let ctx = {
            let sds = lock(&sh.sds);
            endmill_model::pipeline::calibrated_context(&p, Some(&sds)).0
        };
        let v = endmill_model::viewport::build_with_context(&p, &pattern_key, &ctx)?;
        *lock(&sh.viewport_cache) = Some((key, v.clone()));
        Ok(v)
    })
    .await
}

#[derive(Serialize)]
pub struct EnvDerived {
    pub label: String,
    pub dew_point_c: f64,
    pub wet_bulb_c: f64,
    pub humidity_ratio_g_kg: f64,
    pub air_density: f64,
    pub boiling_c: f64,
    pub flood_c: f64,
    pub through_tool_c: f64,
    pub air_blast_c: f64,
    pub mist_c: f64,
    pub drift_um_10min: f64,
    pub condensation: bool,
    pub corrosion_rh: bool,
}

#[derive(Serialize)]
pub struct ModelSettingsView {
    pub auto_download: bool,
    pub startup_download: bool,
    pub endpoint: String,
    pub token_set: bool,
}

#[derive(Serialize)]
pub struct SettingsResponse {
    pub environment: ShopEnvironment,
    pub derived: EnvDerived,
    pub models: ModelSettingsView,
    pub ui: UiSettings,
    pub data_dir: String,
}

fn settings_view(sh: &Shared) -> SettingsResponse {
    let s = lock(&sh.settings).clone();
    let e = &s.environment;
    let flood = CoolantConfig::flood().temperature_in(e);
    let tt = CoolantConfig::through_tool().temperature_in(e);
    SettingsResponse {
        derived: EnvDerived {
            label: e.label(),
            dew_point_c: e.dew_point_c(),
            wet_bulb_c: e.wet_bulb_c(),
            humidity_ratio_g_kg: e.humidity_ratio() * 1000.0,
            air_density: e.air_density(),
            boiling_c: e.boiling_c(),
            flood_c: flood,
            through_tool_c: tt,
            air_blast_c: CoolantConfig::air_blast().temperature_in(e),
            mist_c: CoolantConfig::mist().temperature_in(e),
            drift_um_10min: endmill_model::environment::drift_um(e),
            condensation: flood.min(tt) < e.dew_point_c() + 1.0,
            corrosion_rh: e.humidity_pct >= endmill_model::environment::CORROSION_RH_PCT,
        },
        environment: s.environment.clone(),
        models: ModelSettingsView {
            auto_download: s.models.auto_download,
            startup_download: s.models.startup_download,
            endpoint: if s.models.endpoint.trim().is_empty() { endmill_model::modelhub::DEFAULT_ENDPOINT.into() } else { s.models.endpoint.clone() },
            token_set: !s.models.token.trim().is_empty(),
        },
        ui: s.ui.clone(),
        data_dir: sh.data_dir.display().to_string(),
    }
}

#[tauri::command]
pub async fn settings_get(state: tauri::State<'_, AppState>) -> Result<SettingsResponse, String> {
    blocking(&state, |sh| Ok(settings_view(sh))).await
}

#[tauri::command]
pub async fn settings_set_environment(state: tauri::State<'_, AppState>, environment: ShopEnvironment) -> Result<SettingsResponse, String> {
    blocking(&state, move |sh| {
        environment.validate()?;
        lock(&sh.settings).environment = environment.clone();
        save_settings(sh)?;
        let profiles: Vec<MachiningProfile> = {
            let mut store = lock(&sh.store);
            for p in store.profiles.iter_mut() {
                p.machine.environment = environment.clone();
            }
            store.profiles.clone()
        };
        {
            let mut lib = lock(&sh.library);
            for p in profiles.iter() {
                let _ = lib.save_profile(p, None);
            }
        }
        lock(&sh.ws).set_environment(&environment);
        *lock(&sh.viewport_cache) = None;
        Ok(settings_view(sh))
    })
    .await
}

#[tauri::command]
pub async fn settings_set_models(
    state: tauri::State<'_, AppState>,
    auto_download: bool,
    startup_download: bool,
    endpoint: Option<String>,
    token: Option<String>,
    clear_token: Option<bool>,
) -> Result<SettingsResponse, String> {
    blocking(&state, move |sh| {
        let endpoint = endpoint.map(|e| e.trim().trim_end_matches('/').to_string()).unwrap_or_default();
        if !endpoint.is_empty() && !(endpoint.starts_with("https://") || endpoint.starts_with("http://")) {
            return Err("모델 서버 주소는 http:// 또는 https:// 로 시작해야 합니다".into());
        }
        let (ep, tk) = {
            let mut s = lock(&sh.settings);
            s.models.auto_download = auto_download;
            s.models.startup_download = startup_download;
            s.models.endpoint = endpoint;
            if clear_token.unwrap_or(false) {
                s.models.token.clear();
            } else if let Some(t) = token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
                s.models.token = t;
            }
            (s.models.endpoint.clone(), s.models.token.clone())
        };
        save_settings(sh)?;
        lock(&sh.hub).configure(Some(&ep), Some(&tk));
        Ok(settings_view(sh))
    })
    .await
}

#[tauri::command]
pub async fn settings_set_ui(state: tauri::State<'_, AppState>, ui: UiSettings) -> Result<SettingsResponse, String> {
    blocking(&state, move |sh| {
        lock(&sh.settings).ui = ui;
        save_settings(sh)?;
        Ok(settings_view(sh))
    })
    .await
}

#[derive(Serialize)]
pub struct ModelEntry {
    pub key: String,
    pub label: String,
    pub repo: String,
    pub revision: String,
    pub dir: String,
    pub used_for: String,
    pub fallback: Option<String>,
    pub files: Vec<ModelFile>,
    pub total_bytes: u64,
    pub installed: bool,
    pub detected_dir: Option<String>,
    pub on_disk_bytes: u64,
    pub partial_bytes: u64,
    pub loaded: bool,
    pub loading: bool,
    pub load_detail: String,
    pub load_error: Option<String>,
    pub job: Option<DownloadJob>,
}

#[derive(Serialize)]
pub struct ModelCatalog {
    pub root: String,
    pub device: String,
    pub endpoint: String,
    pub token_set: bool,
    pub auto_download: bool,
    pub startup_download: bool,
    pub any_active: bool,
    pub entries: Vec<ModelEntry>,
}

fn usable_model_dir(models: &Models, key: &str, st: &InstallStatus) -> Option<PathBuf> {
    let own = std::path::Path::new(&st.dir);
    let own_incomplete = !st.installed && (st.partial_bytes > 0 || !own.join("model.safetensors").exists());
    models.detected_dir(key).filter(|d| {
        if !own_incomplete {
            return true;
        }
        let a = fs::canonicalize(d).unwrap_or_else(|_| d.clone());
        let b = fs::canonicalize(own).unwrap_or_else(|_| own.to_path_buf());
        a != b
    })
}

fn catalog_of(sh: &Shared) -> ModelCatalog {
    let snap = lock(&sh.model_snapshot).clone();
    let loading = lock(&sh.loading_model).clone();
    let ms = lock(&sh.settings).models.clone();
    let (root, endpoint, token_set, entries_raw, any_active) = {
        let hub = lock(&sh.hub);
        let raw: Vec<_> = endmill_model::modelhub::catalog()
            .into_iter()
            .map(|spec| {
                let st = endmill_model::modelhub::install_status(&hub.root, &spec);
                let job = hub.job(&spec.key);
                (spec, st, job)
            })
            .collect();
        (hub.root.clone(), hub.endpoint.clone(), hub.token.is_some(), raw, hub.any_active())
    };
    let detected: Vec<Option<String>> = match sh.models.try_lock() {
        Ok(m) => entries_raw.iter().map(|(s, st, _)| usable_model_dir(&m, &s.key, st).map(|d| d.display().to_string())).collect(),
        Err(_) => entries_raw.iter().map(|(_, st, _)| if st.installed { Some(st.dir.clone()) } else { None }).collect(),
    };
    let entries = entries_raw
        .into_iter()
        .zip(detected)
        .map(|((spec, st, job), det)| {
            let slot = snap.slots.iter().find(|s| s.key == spec.key);
            ModelEntry {
                key: spec.key.clone(),
                label: spec.label.clone(),
                repo: spec.repo.clone(),
                revision: spec.revision.clone(),
                dir: st.dir.clone(),
                used_for: spec.used_for.clone(),
                fallback: spec.fallback.clone(),
                total_bytes: spec.total_bytes(),
                files: spec.files.clone(),
                installed: st.installed,
                detected_dir: det,
                on_disk_bytes: st.on_disk_bytes,
                partial_bytes: st.partial_bytes,
                loaded: slot.map(|s| s.loaded).unwrap_or(false),
                loading: loading.as_deref() == Some(spec.key.as_str()),
                load_detail: slot.map(|s| s.detail.clone()).unwrap_or_default(),
                load_error: slot.and_then(|s| s.error.clone()),
                job,
            }
        })
        .collect();
    ModelCatalog {
        root: root.display().to_string(),
        device: snap.device.clone(),
        endpoint,
        token_set,
        auto_download: ms.auto_download,
        startup_download: ms.startup_download,
        any_active,
        entries,
    }
}

#[tauri::command]
pub async fn models_catalog(state: tauri::State<'_, AppState>) -> Result<ModelCatalog, String> {
    blocking(&state, |sh| Ok(catalog_of(sh))).await
}

#[tauri::command]
pub async fn models_download(state: tauri::State<'_, AppState>, key: String) -> Result<ModelCatalog, String> {
    blocking(&state, move |sh| {
        lock(&sh.hub).start(&key)?;
        Ok(catalog_of(sh))
    })
    .await
}

#[tauri::command]
pub async fn models_cancel(state: tauri::State<'_, AppState>, key: String) -> Result<ModelCatalog, String> {
    blocking(&state, move |sh| {
        lock(&sh.hub).cancel(&key);
        Ok(catalog_of(sh))
    })
    .await
}

#[tauri::command]
pub async fn models_unload(state: tauri::State<'_, AppState>, key: String) -> Result<ModelCatalog, String> {
    blocking(&state, move |sh| {
        {
            let mut models = lock(&sh.models);
            models.unload(&key);
            *lock(&sh.model_snapshot) = models.status();
        }
        Ok(catalog_of(sh))
    })
    .await
}

#[tauri::command]
pub async fn models_delete(state: tauri::State<'_, AppState>, key: String) -> Result<ModelCatalog, String> {
    blocking(&state, move |sh| {
        {
            let mut models = lock(&sh.models);
            models.unload(&key);
            *lock(&sh.model_snapshot) = models.status();
        }
        lock(&sh.hub).delete(&key)?;
        Ok(catalog_of(sh))
    })
    .await
}

#[derive(Serialize)]
pub struct EnsureResponse {
    pub key: String,
    pub state: String,
    pub detail: String,
    pub job: Option<DownloadJob>,
}

#[tauri::command]
pub async fn models_ensure(state: tauri::State<'_, AppState>, key: String, download: Option<bool>) -> Result<EnsureResponse, String> {
    blocking(&state, move |sh| {
        if endmill_model::modelhub::spec_of(&key).is_none() {
            return Err(format!("알 수 없는 모델: {}", key));
        }
        let respond = |state: &str, detail: String, job: Option<DownloadJob>| EnsureResponse { key: key.clone(), state: state.into(), detail, job };
        let job = lock(&sh.hub).job(&key);
        if let Some(j) = job.as_ref().filter(|j| j.active()) {
            return Ok(respond("downloading", format!("{:.1}%", j.percent()), Some(j.clone())));
        }
        let own = lock(&sh.hub).status(&key);
        let mut models = lock(&sh.models);
        if models.is_loaded(&key) {
            return Ok(respond("ready", "로드됨".into(), job));
        }
        let usable = match own.as_ref() {
            Some(st) => usable_model_dir(&models, &key, st),
            None => models.detected_dir(&key),
        };
        if usable.is_some() {
            *lock(&sh.loading_model) = Some(key.clone());
            let r = models.load(&key);
            *lock(&sh.loading_model) = None;
            *lock(&sh.model_snapshot) = models.status();
            return Ok(match r {
                Ok(d) => respond("ready", d, job),
                Err(e) => respond("failed", e, job),
            });
        }
        drop(models);
        if let Some(j) = job.as_ref().filter(|j| j.state == "failed") {
            if !download.unwrap_or(false) {
                return Ok(respond("failed", j.error.clone().unwrap_or_default(), Some(j.clone())));
            }
        }
        let auto = lock(&sh.settings).models.auto_download;
        if download.unwrap_or(auto) {
            let j = lock(&sh.hub).start(&key)?;
            return Ok(respond("downloading", "다운로드 시작".into(), Some(j)));
        }
        Ok(respond("missing", "모델 파일이 없습니다 (설정 > 모델에서 다운로드)".into(), job))
    })
    .await
}

#[tauri::command]
pub fn ingest_needs(name: String, options: Option<IngestOptions>) -> Vec<String> {
    endmill_model::pipeline::ingest_model_needs(&name, &options.unwrap_or_default())
}
