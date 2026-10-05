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

pub struct Shared {
    pub data_dir: PathBuf,
    pub store: Mutex<ProfileStore>,
    pub ws: Mutex<Workspace>,
    pub models: Mutex<Models>,
    pub sds: Mutex<SdsStore>,
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
        let store_path = data_dir.join("profiles.json");
        let store = ProfileStore::load_from_file(&store_path.display().to_string())
            .ok()
            .filter(|s| !s.profiles.is_empty())
            .unwrap_or_else(build_demo_profile_store);
        let first = store
            .profiles
            .first()
            .cloned()
            .unwrap_or_else(|| MachiningProfile::from_preset(&ProfileStore::new().presets[0], "기본"));
        let models_root = fs::read_to_string(data_dir.join("models_root.txt"))
            .map(|s| PathBuf::from(s.trim()))
            .unwrap_or_else(|_| data_dir.join("models"));
        Self {
            shared: Arc::new(Shared {
                store: Mutex::new(store),
                ws: Mutex::new(Workspace::new(&data_dir, first)),
                models: Mutex::new(Models::new(&models_root)),
                sds: Mutex::new(SdsStore::open(&data_dir.join("sds"))),
                data_dir,
            }),
        }
    }

    pub fn flush(&self) {
        lock(&self.shared.sds).flush();
        persist_store(&self.shared);
    }
}

fn persist_store(sh: &Shared) {
    let store = lock(&sh.store);
    let _ = store.save_to_file(&sh.data_dir.join("profiles.json").display().to_string());
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
    persist_store(sh);
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
    vec![
        EndMillMockupSetting::default_10mm_4flute(),
        EndMillMockupSetting::default_6mm_2flute_aluminum(),
        EndMillMockupSetting::default_12mm_ball_nose(),
    ]
    .iter()
    .map(endmill_to_response)
    .collect()
}

#[tauri::command]
pub fn list_presets() -> Vec<PresetResponse> {
    let store = ProfileStore::new();
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
    let profile: MachiningProfile =
        serde_json::from_str(&json).map_err(|e| format!("역직렬화 실패: {}", e))?;
    profile.endmill_setting.validate()?;
    sync_profile_to_store(&state.shared, &profile);
    Ok(profile_to_response(&profile))
}

#[tauri::command]
pub fn list_workpiece_setups() -> Vec<WorkpieceSetupResponse> {
    vec![
        WorkpieceSetup::default_aluminum_plate(),
        WorkpieceSetup::default_steel_block(),
        WorkpieceSetup::default_titanium_plate(),
        WorkpieceSetup::default_cylindrical_steel(),
        WorkpieceSetup::default_inconel_block(),
    ]
    .iter()
    .map(workpiece_to_response)
    .collect()
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
pub fn apply_coolant_to_profile(
    state: tauri::State<'_, AppState>,
    profile_name: String,
    coolant_key: String,
) -> Result<CoolantOptionResponse, String> {
    let cfg = CoolantConfig::from_key(&coolant_key)
        .ok_or_else(|| format!("지원하지 않는 냉각: {}", coolant_key))?;
    let mut profile = with_profile(&state, &profile_name, |p| Ok(p.clone()))?;
    profile.conditions.coolant = endmill_model::pipeline::coolant_type_of(&cfg.method);
    profile.coolant_config = cfg.clone();
    sync_profile_to_store(&state.shared, &profile);
    if let Ok(mut ws) = state.shared.ws.try_lock() {
        if ws.profile.name == profile.name {
            ws.set_profile(profile);
        }
    }
    Ok(coolant_to_response(&coolant_key, &cfg))
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

    let profile = {
        let mut store = lock(&state.shared.store);
        store
            .create_profile_from_custom(name, description, endmill, conditions, workpiece, coolant, tags)?
            .clone()
    };
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
            let models = lock(&sh.models);
            let mut sds = lock(&sh.sds);
            let out = ws.ingest(&models, &mut sds, &name, &bytes, &opt)?;
            (out, ws.profile.clone())
        };
        if !out.doc.applied.is_empty() {
            sync_profile_to_store(sh, &profile);
        }
        Ok(out)
    })
    .await
}

#[tauri::command]
pub async fn ws_run_process(state: tauri::State<'_, AppState>) -> Result<ProcessOutcome, String> {
    blocking(&state, |sh| {
        let mut ws = lock(&sh.ws);
        let sds = lock(&sh.sds);
        ws.run_process(&sds)
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
        ws.decide(&models, &mut sds, use_advisor)
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
        let _ = fs::write(sh.data_dir.join("models_root.txt"), dir.display().to_string());
        let mut models = lock(&sh.models);
        models.set_root(&dir);
        Ok(models.status())
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
            let _ = match k {
                "siglip" => models.load_siglip(),
                "ttm" => models.load_ttm(),
                _ => models.load_laya(),
            };
        }
        Ok(models.status())
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