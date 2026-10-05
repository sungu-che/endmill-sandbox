use serde::{Deserialize, Serialize};
use crate::cutting::CuttingConditions;
use crate::workpiece_setup::WorkpieceSetup;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CoolantMethod {
    AirBlast,
    Flood,
    Mist,
    ThroughTool,
    Dry,
}

impl CoolantMethod {
    pub fn key(&self) -> &'static str {
        match self {
            Self::AirBlast => "air_blast",
            Self::Flood => "flood",
            Self::Mist => "mist",
            Self::ThroughTool => "through_tool",
            Self::Dry => "dry",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_lowercase().replace(['-', ' '], "_").as_str() {
            "air_blast" | "air" | "airblast" | "에어" | "에어블로우" | "에어_블로우" => Some(Self::AirBlast),
            "flood" | "wet" | "emulsion" | "플러드" | "수용성" => Some(Self::Flood),
            "mist" | "mql" | "미스트" => Some(Self::Mist),
            "through_tool" | "throughtool" | "tsc" | "through_spindle" | "내부급유" | "내부_급유" => Some(Self::ThroughTool),
            "dry" | "none" | "건식" => Some(Self::Dry),
            _ => None,
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::AirBlast => "에어 블로우".into(),
            Self::Flood => "플러드 (수성 냉각수)".into(),
            Self::Mist => "미스트 (MQL)".into(),
            Self::ThroughTool => "내부 급유 (Through-tool)".into(),
            Self::Dry => "건식".into(),
        }
    }

    pub fn gcode_m_code(&self) -> &str {
        match self {
            Self::Flood => "M08",
            Self::Mist => "M07",
            Self::AirBlast => "M83",
            Self::ThroughTool => "M88",
            Self::Dry => "M09",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoolantConfig {
    pub method: CoolantMethod,
    pub pressure_bar: f64,
    pub flow_rate_l_min: f64,
    pub nozzle_count: u8,
    pub nozzle_angle_deg: f64,
    pub nozzle_distance_mm: f64,
    pub temperature_c: Option<f64>,
}

impl CoolantConfig {
    pub fn from_method(method: &CoolantMethod) -> Self {
        match method {
            CoolantMethod::AirBlast => Self::air_blast(),
            CoolantMethod::Flood => Self::flood(),
            CoolantMethod::Mist => Self::mist(),
            CoolantMethod::ThroughTool => Self::through_tool(),
            CoolantMethod::Dry => Self::dry(),
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        CoolantMethod::from_key(key).map(|m| Self::from_method(&m))
    }

    pub fn air_blast() -> Self {
        Self {
            method: CoolantMethod::AirBlast,
            pressure_bar: 6.0,
            flow_rate_l_min: 0.0,
            nozzle_count: 1,
            nozzle_angle_deg: 45.0,
            nozzle_distance_mm: 30.0,
            temperature_c: None,
        }
    }

    pub fn flood() -> Self {
        Self {
            method: CoolantMethod::Flood,
            pressure_bar: 2.0,
            flow_rate_l_min: 12.0,
            nozzle_count: 2,
            nozzle_angle_deg: 30.0,
            nozzle_distance_mm: 50.0,
            temperature_c: Some(22.0),
        }
    }

    pub fn mist() -> Self {
        Self {
            method: CoolantMethod::Mist,
            pressure_bar: 4.0,
            flow_rate_l_min: 0.05,
            nozzle_count: 1,
            nozzle_angle_deg: 45.0,
            nozzle_distance_mm: 40.0,
            temperature_c: None,
        }
    }

    pub fn through_tool() -> Self {
        Self {
            method: CoolantMethod::ThroughTool,
            pressure_bar: 20.0,
            flow_rate_l_min: 8.0,
            nozzle_count: 0,
            nozzle_angle_deg: 0.0,
            nozzle_distance_mm: 0.0,
            temperature_c: Some(25.0),
        }
    }

    pub fn dry() -> Self {
        Self {
            method: CoolantMethod::Dry,
            pressure_bar: 0.0,
            flow_rate_l_min: 0.0,
            nozzle_count: 0,
            nozzle_angle_deg: 0.0,
            nozzle_distance_mm: 0.0,
            temperature_c: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum ToolNose {
    #[default]
    Square,
    Ball,
    CornerRadius { radius_mm: f64 },
}

impl ToolNose {
    pub fn key(&self) -> String {
        match self {
            Self::Square => "square".into(),
            Self::Ball => "ball".into(),
            Self::CornerRadius { radius_mm } => format!("cr{:.2}", radius_mm),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Square => "스퀘어".into(),
            Self::Ball => "볼노즈".into(),
            Self::CornerRadius { radius_mm } => format!("코너R {:.2}", radius_mm),
        }
    }

    pub fn corner_radius(&self, diameter_mm: f64) -> f64 {
        match self {
            Self::Square => 0.0,
            Self::Ball => diameter_mm / 2.0,
            Self::CornerRadius { radius_mm } => radius_mm.min(diameter_mm / 2.0).max(0.0),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndMillMockupSetting {
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
    #[serde(default)]
    pub nose: ToolNose,
    #[serde(default)]
    pub stickout_mm: Option<f64>,
    #[serde(default)]
    pub runout_um: Option<f64>,
    #[serde(default)]
    pub variable_pitch: bool,
}

impl EndMillMockupSetting {
    pub fn custom(
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
    ) -> Self {
        Self {
            name,
            model,
            diameter_mm,
            flute_count,
            loc_mm,
            oal_mm,
            shank_diameter_mm,
            helix_angle_deg,
            coating_name,
            is_high_end,
            nose: ToolNose::Square,
            stickout_mm: None,
            runout_um: None,
            variable_pitch: false,
        }
    }

    pub fn with_nose(mut self, nose: ToolNose) -> Self {
        self.nose = nose;
        self
    }

    pub fn with_stickout(mut self, stickout_mm: Option<f64>) -> Self {
        self.stickout_mm = stickout_mm.filter(|v| *v > 0.0);
        self
    }

    pub fn signature(&self) -> String {
        format!(
            "D{:.1}Z{}-{}-{}",
            self.diameter_mm,
            self.flute_count,
            self.nose.key(),
            self.coating_name.clone().unwrap_or_else(|| "bare".into()).to_lowercase().replace(' ', "_")
        )
    }

    pub fn effective_stickout_mm(&self) -> f64 {
        if let Some(s) = self.stickout_mm.filter(|v| *v > 0.0) {
            return s.max(self.loc_mm);
        }
        let grip = (self.oal_mm - self.loc_mm - 0.2 * self.diameter_mm).min(4.0 * self.shank_diameter_mm);
        if grip > 0.0 {
            (self.oal_mm - grip).max(self.loc_mm + 0.5 * self.diameter_mm)
        } else {
            self.loc_mm + 10.0
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.diameter_mm <= 0.0 {
            return Err("직경은 0보다 커야 합니다".into());
        }
        if self.flute_count < 1 || self.flute_count > 12 {
            return Err("날 수는 1~12 범위여야 합니다".into());
        }
        if self.loc_mm <= 0.0 {
            return Err("유효 절삭 길이는 0보다 커야 합니다".into());
        }
        if self.oal_mm < self.loc_mm {
            return Err("전체 길이는 유효 절삭 길이보다 커야 합니다".into());
        }
        if self.shank_diameter_mm <= 0.0 {
            return Err("샹크 직경은 0보다 커야 합니다".into());
        }
        if self.helix_angle_deg < 0.0 || self.helix_angle_deg > 60.0 {
            return Err("헬릭스 각도는 0~60° 범위여야 합니다".into());
        }
        if let ToolNose::CornerRadius { radius_mm } = self.nose {
            if radius_mm <= 0.0 || radius_mm > self.diameter_mm / 2.0 {
                return Err("코너 R 은 0 보다 크고 반경 이하여야 합니다".into());
            }
        }
        if let Some(s) = self.stickout_mm {
            if s < self.loc_mm {
                return Err("돌출 길이는 유효 절삭 길이 이상이어야 합니다".into());
            }
            if s > self.oal_mm {
                return Err("돌출 길이는 전체 길이 이하여야 합니다".into());
            }
        }
        if let Some(r) = self.runout_um {
            if !(0.0..=200.0).contains(&r) {
                return Err("런아웃은 0~200 µm 범위여야 합니다".into());
            }
        }
        Ok(())
    }
    
    pub fn default_10mm_4flute() -> Self {
        Self {
            name: "Ø10 4날 (기본)".into(),
            model: "OSG ADO-SUS-4D".into(),
            diameter_mm: 10.0,
            flute_count: 4,
            loc_mm: 25.0,
            oal_mm: 75.0,
            shank_diameter_mm: 10.0,
            helix_angle_deg: 40.0,
            coating_name: Some("nACo HiPIMS".into()),
            is_high_end: true,
            nose: ToolNose::Square,
            stickout_mm: None,
            runout_um: None,
            variable_pitch: true,
        }
    }

    pub fn default_6mm_2flute_aluminum() -> Self {
        Self {
            name: "Ø6 2날 알루미늄용".into(),
            model: "YG-1 AL-2F".into(),
            diameter_mm: 6.0,
            flute_count: 2,
            loc_mm: 15.0,
            oal_mm: 60.0,
            shank_diameter_mm: 6.0,
            helix_angle_deg: 35.0,
            coating_name: Some("TiAlN".into()),
            is_high_end: false,
            nose: ToolNose::Square,
            stickout_mm: None,
            runout_um: None,
            variable_pitch: false,
        }
    }

    pub fn default_12mm_ball_nose() -> Self {
        Self {
            name: "Ø12 볼노즈".into(),
            model: "Sandvik R216.34".into(),
            diameter_mm: 12.0,
            flute_count: 2,
            loc_mm: 30.0,
            oal_mm: 80.0,
            shank_diameter_mm: 12.0,
            helix_angle_deg: 30.0,
            coating_name: Some("AlTiN".into()),
            is_high_end: true,
            nose: ToolNose::Ball,
            stickout_mm: None,
            runout_um: None,
            variable_pitch: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachiningPreset {
    pub name: String,
    pub description: String,
    pub endmill_setting: EndMillMockupSetting,
    pub conditions: CuttingConditions,
    pub workpiece_setup: WorkpieceSetup,
    pub coolant_config: CoolantConfig,
}

impl MachiningPreset {
    pub fn default_aluminum_roughing() -> Self {
        let endmill = EndMillMockupSetting::default_6mm_2flute_aluminum();
        let conditions = crate::cutting::CuttingCalculator::recommend_conditions(
            &crate::cutting::WorkpieceMaterial::Aluminum,
            endmill.diameter_mm,
            endmill.flute_count,
            endmill.is_high_end,
        ).unwrap_or(CuttingConditions {
            cutting_speed_m_min: 300.0,
            feed_rate_mm_min: 3000.0,
            feed_per_tooth_mm: 0.18,
            axial_doc_mm: 6.0,
            radial_doc_mm: 1.8,
            spindle_rpm: 15915,
            coolant: crate::cutting::CoolantType::Flood,
        });

        Self {
            name: "알루미늄 황삭 프리셋".into(),
            description: "알루미늄 합금 황삭 가공용 기본 설정".into(),
            endmill_setting: endmill,
            conditions,
            workpiece_setup: WorkpieceSetup::default_aluminum_plate(),
            coolant_config: CoolantConfig::flood(),
        }
    }

    pub fn default_titanium_finishing() -> Self {
        let endmill = EndMillMockupSetting::default_10mm_4flute();
        let conditions = crate::cutting::CuttingCalculator::recommend_conditions(
            &crate::cutting::WorkpieceMaterial::Titanium,
            endmill.diameter_mm,
            endmill.flute_count,
            true,
        ).unwrap_or(CuttingConditions {
            cutting_speed_m_min: 60.0,
            feed_rate_mm_min: 600.0,
            feed_per_tooth_mm: 0.05,
            axial_doc_mm: 10.0,
            radial_doc_mm: 3.0,
            spindle_rpm: 1910,
            coolant: crate::cutting::CoolantType::ThroughTool,
        });

        Self {
            name: "티타늄 정삭 프리셋".into(),
            description: "Ti-6Al-4V 정삭 가공용 고급 설정".into(),
            endmill_setting: endmill,
            conditions,
            workpiece_setup: WorkpieceSetup::default_titanium_plate(),
            coolant_config: CoolantConfig::through_tool(),
        }
    }

    pub fn default_steel_general() -> Self {
        let endmill = EndMillMockupSetting::default_10mm_4flute();
        let conditions = crate::cutting::CuttingCalculator::recommend_conditions(
            &crate::cutting::WorkpieceMaterial::CarbonSteel,
            endmill.diameter_mm,
            endmill.flute_count,
            endmill.is_high_end,
        ).unwrap_or(CuttingConditions {
            cutting_speed_m_min: 180.0,
            feed_rate_mm_min: 1500.0,
            feed_per_tooth_mm: 0.1,
            axial_doc_mm: 10.0,
            radial_doc_mm: 3.0,
            spindle_rpm: 5730,
            coolant: crate::cutting::CoolantType::Mist,
        });

        Self {
            name: "탄소강 범용 프리셋".into(),
            description: "S45C 범용 가공용 설정".into(),
            endmill_setting: endmill,
            conditions,
            workpiece_setup: WorkpieceSetup::default_steel_block(),
            coolant_config: CoolantConfig::mist(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineLimits {
    pub max_rpm: f64,
    pub base_rpm: f64,
    pub max_power_kw: f64,
    pub max_feed_mm_min: f64,
    pub rapid_mm_min: f64,
    pub efficiency: f64,
    pub holder_stiffness_n_per_um: f64,
    pub spindle_drift_um_per_hr: f64,
}

impl Default for MachineLimits {
    fn default() -> Self {
        Self {
            max_rpm: 12000.0,
            base_rpm: 1500.0,
            max_power_kw: 7.5,
            max_feed_mm_min: 10000.0,
            rapid_mm_min: 15000.0,
            efficiency: 0.8,
            holder_stiffness_n_per_um: 50.0,
            spindle_drift_um_per_hr: 0.0,
        }
    }
}

impl MachineLimits {
    pub fn available_power_kw(&self, rpm: f64) -> f64 {
        if self.base_rpm <= 0.0 {
            return self.max_power_kw;
        }
        self.max_power_kw * (rpm / self.base_rpm).clamp(0.0, 1.0)
    }

    pub fn max_torque_nm(&self) -> f64 {
        self.max_power_kw * 1000.0 * 60.0 / (2.0 * std::f64::consts::PI * self.base_rpm.max(1.0))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachiningProfile {
    pub name: String,
    pub description: String,
    pub endmill_setting: EndMillMockupSetting,
    pub conditions: CuttingConditions,
    pub workpiece_setup: WorkpieceSetup,
    pub coolant_config: CoolantConfig,
    pub is_default: bool,
    pub created_at: String,
    pub modified_at: String,
    pub tags: Vec<String>,
    #[serde(default)]
    pub machine: MachineLimits,
}

impl MachiningProfile {
    pub fn from_preset(preset: &MachiningPreset, name: &str) -> Self {
        let now = chrono_now();
        Self {
            name: name.to_string(),
            description: preset.description.clone(),
            endmill_setting: preset.endmill_setting.clone(),
            conditions: preset.conditions.clone(),
            workpiece_setup: preset.workpiece_setup.clone(),
            coolant_config: preset.coolant_config.clone(),
            is_default: false,
            created_at: now.clone(),
            modified_at: now,
            tags: Vec::new(),
            machine: MachineLimits::default(),
        }
    }
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{}", secs)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileStore {
    pub profiles: Vec<MachiningProfile>,
    pub presets: Vec<MachiningPreset>,
}

impl ProfileStore {
    pub fn new() -> Self {
        let presets = vec![
            MachiningPreset::default_aluminum_roughing(),
            MachiningPreset::default_titanium_finishing(),
            MachiningPreset::default_steel_general(),
        ];
        Self {
            profiles: Vec::new(),
            presets,
        }
    }

    pub fn add_profile(&mut self, profile: MachiningProfile) {
        self.profiles.push(profile);
    }

    pub fn get_by_name(&self, name: &str) -> Option<&MachiningProfile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    pub fn upsert_profile(&mut self, profile: MachiningProfile) {
        match self.profiles.iter_mut().find(|p| p.name == profile.name) {
            Some(slot) => *slot = profile,
            None => self.profiles.push(profile),
        }
    }

    pub fn remove_by_name(&mut self, name: &str) -> bool {
        let len_before = self.profiles.len();
        self.profiles.retain(|p| p.name != name);
        self.profiles.len() < len_before
    }

    pub fn get_preset_by_name(&self, name: &str) -> Option<&MachiningPreset> {
        self.presets.iter().find(|p| p.name == name)
    }

    pub fn add_preset(&mut self, preset: MachiningPreset) {
        self.presets.push(preset);
    }

    pub fn remove_preset_by_name(&mut self, name: &str) -> bool {
        let len_before = self.presets.len();
        self.presets.retain(|p| p.name != name);
        self.presets.len() < len_before
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }

    pub fn save_to_file(&self, path: &str) -> Result<(), String> {
        let json = self.to_json();
        std::fs::write(path, json).map_err(|e| format!("파일 쓰기 실패: {}", e))
    }

    pub fn load_from_file(path: &str) -> Result<Self, String> {
        let json = std::fs::read_to_string(path)
            .map_err(|e| format!("파일 읽기 실패: {}", e))?;
        Self::from_json(&json)
    }

    pub fn create_profile_from_custom(
        &mut self,
        name: String,
        description: String,
        endmill: EndMillMockupSetting,
        conditions: crate::cutting::CuttingConditions,
        workpiece: crate::workpiece_setup::WorkpieceSetup,
        coolant: CoolantConfig,
        tags: Vec<String>,
    ) -> Result<&MachiningProfile, String> {
        endmill.validate()?;
        let now = chrono_now();
        let profile = MachiningProfile {
            name,
            description,
            endmill_setting: endmill,
            conditions,
            workpiece_setup: workpiece,
            coolant_config: coolant,
            is_default: false,
            created_at: now.clone(),
            modified_at: now,
            tags,
            machine: MachineLimits::default(),
        };
        self.upsert_profile(profile.clone());
        Ok(self.profiles.iter().find(|p| p.name == profile.name).unwrap())
    }
}