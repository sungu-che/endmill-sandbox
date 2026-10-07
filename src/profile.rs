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

pub const STANDARD_AMBIENT_C: f64 = 22.0;
pub const STANDARD_HUMIDITY_PCT: f64 = 50.0;
pub const STANDARD_PRESSURE_KPA: f64 = 101.325;
pub const SUMP_K_PER_W: f64 = 3.0 / (20.0e5 * 8.0 / 60000.0 / 0.5);
pub const JOULE_THOMSON_K_PER_BAR: f64 = 0.25;
pub const SUMP_EVAPORATION_PULL: f64 = 0.15;

fn default_ambient_c() -> f64 {
    STANDARD_AMBIENT_C
}

fn default_humidity_pct() -> f64 {
    STANDARD_HUMIDITY_PCT
}

fn default_reference_c() -> f64 {
    20.0
}

fn default_swing_c_per_h() -> f64 {
    0.5
}

fn default_pressure_kpa() -> f64 {
    STANDARD_PRESSURE_KPA
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShopEnvironment {
    #[serde(default = "default_ambient_c")]
    pub ambient_c: f64,
    #[serde(default = "default_humidity_pct")]
    pub humidity_pct: f64,
    #[serde(default = "default_reference_c")]
    pub reference_c: f64,
    #[serde(default = "default_swing_c_per_h")]
    pub swing_c_per_h: f64,
    #[serde(default)]
    pub chiller_c: Option<f64>,
    #[serde(default = "default_pressure_kpa")]
    pub pressure_kpa: f64,
}

impl Default for ShopEnvironment {
    fn default() -> Self {
        Self {
            ambient_c: STANDARD_AMBIENT_C,
            humidity_pct: STANDARD_HUMIDITY_PCT,
            reference_c: default_reference_c(),
            swing_c_per_h: default_swing_c_per_h(),
            chiller_c: None,
            pressure_kpa: STANDARD_PRESSURE_KPA,
        }
    }
}

pub fn saturation_kpa(t_c: f64) -> f64 {
    0.61121 * ((18.678 - t_c / 234.5) * (t_c / (257.14 + t_c))).exp()
}

pub fn boiling_point_c(pressure_kpa: f64) -> f64 {
    let p = pressure_kpa.clamp(20.0, 400.0);
    let mut t: f64 = 100.0;
    for _ in 0..30 {
        let f = saturation_kpa(t) - p;
        let h = 1e-3;
        let df = (saturation_kpa(t + h) - saturation_kpa(t - h)) / (2.0 * h);
        let step = f / df.max(1e-9);
        t -= step;
        if step.abs() < 1e-9 {
            break;
        }
    }
    t
}

fn sutherland(t_k: f64, ref_val: f64, t_ref: f64, s: f64) -> f64 {
    ref_val * (t_k / t_ref).powf(1.5) * (t_ref + s) / (t_k + s)
}

pub fn water_viscosity_pa_s(t_c: f64) -> f64 {
    2.414e-5 * 10f64.powf(247.8 / (t_c.clamp(0.0, 99.0) + 273.15 - 140.0))
}

pub fn water_conductivity(t_c: f64) -> f64 {
    let t = t_c.clamp(0.0, 99.0);
    0.5706 + 1.756e-3 * t - 6.46e-6 * t * t
}

impl ShopEnvironment {
    pub fn standard() -> Self {
        Self::default()
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(self.ambient_c.is_finite() && (0.0..=50.0).contains(&self.ambient_c)) {
            return Err("실내 온도는 0~50 °C 범위로 입력하세요".into());
        }
        if !(self.humidity_pct.is_finite() && (5.0..=98.0).contains(&self.humidity_pct)) {
            return Err("상대 습도는 5~98 % 범위로 입력하세요".into());
        }
        if !(self.reference_c.is_finite() && (15.0..=25.0).contains(&self.reference_c)) {
            return Err("측정 기준 온도는 15~25 °C 범위로 입력하세요 (ISO 1: 20 °C)".into());
        }
        if !(self.swing_c_per_h.is_finite() && (0.0..=10.0).contains(&self.swing_c_per_h)) {
            return Err("실내 온도 변동은 0~10 °C/h 범위로 입력하세요".into());
        }
        if let Some(c) = self.chiller_c {
            if !(c.is_finite() && (5.0..=40.0).contains(&c)) {
                return Err("칠러 설정 온도는 5~40 °C 범위로 입력하세요".into());
            }
        }
        if !(self.pressure_kpa.is_finite() && (60.0..=110.0).contains(&self.pressure_kpa)) {
            return Err("대기압은 60~110 kPa 범위로 입력하세요".into());
        }
        Ok(())
    }

    pub fn rh(&self) -> f64 {
        (self.humidity_pct / 100.0).clamp(0.01, 1.0)
    }

    pub fn vapor_kpa(&self) -> f64 {
        self.rh() * saturation_kpa(self.ambient_c)
    }

    pub fn dew_point_c(&self) -> f64 {
        let (b, c) = (17.62, 243.12);
        let g = self.rh().ln() + b * self.ambient_c / (c + self.ambient_c);
        c * g / (b - g)
    }

    pub fn wet_bulb_at(t: f64, rh_pct: f64) -> f64 {
        let rh = rh_pct.clamp(5.0, 99.0);
        t * (0.151977 * (rh + 8.313659).sqrt()).atan() + (t + rh).atan() - (rh - 1.676331).atan()
            + 0.00391838 * rh.powf(1.5) * (0.023101 * rh).atan()
            - 4.686035
    }

    pub fn wet_bulb_c(&self) -> f64 {
        Self::wet_bulb_at(self.ambient_c, self.humidity_pct).min(self.ambient_c)
    }

    pub fn wet_bulb_depression(&self) -> f64 {
        (self.ambient_c - self.wet_bulb_c()).max(0.0)
    }

    pub fn standard_depression(&self) -> f64 {
        (self.ambient_c - Self::wet_bulb_at(self.ambient_c, STANDARD_HUMIDITY_PCT).min(self.ambient_c)).max(0.0)
    }

    pub fn humidity_ratio(&self) -> f64 {
        let pv = self.vapor_kpa();
        0.621945 * pv / (self.pressure_kpa - pv).max(1.0)
    }

    pub fn air_density(&self) -> f64 {
        Self::air_density_at(self.ambient_c, self.vapor_kpa(), self.pressure_kpa)
    }

    fn air_density_at(t_c: f64, pv_kpa: f64, p_kpa: f64) -> f64 {
        let t = t_c + 273.15;
        (p_kpa - pv_kpa) * 1000.0 / (287.058 * t) + pv_kpa * 1000.0 / (461.495 * t)
    }

    pub fn air_cp(&self) -> f64 {
        let w = self.humidity_ratio();
        (1006.0 + 1860.0 * w) / (1.0 + w)
    }

    pub fn air_viscosity(&self) -> f64 {
        sutherland(self.ambient_c + 273.15, 1.716e-5, 273.15, 110.4) * (1.0 - 0.46 * self.vapor_kpa() / self.pressure_kpa)
    }

    pub fn air_conductivity(&self) -> f64 {
        sutherland(self.ambient_c + 273.15, 0.02414, 273.15, 194.0) * (1.0 - 0.28 * self.vapor_kpa() / self.pressure_kpa)
    }

    pub fn air_kinematic(&self) -> f64 {
        self.air_viscosity() / self.air_density().max(0.1)
    }

    pub fn air_prandtl(&self) -> f64 {
        self.air_viscosity() * self.air_cp() / self.air_conductivity().max(1e-6)
    }

    pub fn boiling_c(&self) -> f64 {
        100.0 + boiling_point_c(self.pressure_kpa) - boiling_point_c(STANDARD_PRESSURE_KPA)
    }

    pub fn label(&self) -> String {
        format!(
            "실내 {:.1} °C · RH {:.0} % · 이슬점 {:.1} °C{}",
            self.ambient_c,
            self.humidity_pct,
            self.dew_point_c(),
            self.chiller_c.map(|c| format!(" · 칠러 {:.1} °C", c)).unwrap_or_default()
        )
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
    pub fn is_liquid(&self) -> bool {
        matches!(self.method, CoolantMethod::Flood | CoolantMethod::ThroughTool)
    }

    pub fn pump_heat_w(&self) -> f64 {
        if !self.is_liquid() {
            return 0.0;
        }
        self.pressure_bar.max(0.0) * 1.0e5 * self.flow_rate_l_min.max(0.0) / 60000.0 / 0.5
    }

    pub fn auto_temperature_c(&self, env: &ShopEnvironment) -> f64 {
        match self.method {
            CoolantMethod::Flood | CoolantMethod::ThroughTool => {
                if let Some(ch) = env.chiller_c {
                    return ch;
                }
                let rise = (self.pump_heat_w() * SUMP_K_PER_W).clamp(0.0, 8.0);
                let evap = SUMP_EVAPORATION_PULL * (env.wet_bulb_depression() - env.standard_depression());
                env.ambient_c + rise - evap
            }
            CoolantMethod::AirBlast | CoolantMethod::Mist => env.ambient_c - JOULE_THOMSON_K_PER_BAR * self.pressure_bar.max(0.0),
            CoolantMethod::Dry => env.ambient_c,
        }
    }

    pub fn temperature_in(&self, env: &ShopEnvironment) -> f64 {
        self.temperature_c.unwrap_or_else(|| self.auto_temperature_c(env))
    }

    pub fn normalize_legacy(&mut self) {
        let legacy = match (&self.method, self.temperature_c) {
            (CoolantMethod::Flood, Some(t)) => (t - 22.0).abs() < 1e-9,
            (CoolantMethod::ThroughTool, Some(t)) => (t - 25.0).abs() < 1e-9,
            _ => false,
        };
        if legacy {
            self.temperature_c = None;
        }
    }

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
            temperature_c: None,
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
            temperature_c: None,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ToolSubstrate {
    #[default]
    Carbide,
    CarbideUltraFine,
    Hss,
    HssCobalt,
    Cbn,
    Pcd,
}

impl ToolSubstrate {
    pub const ALL: [ToolSubstrate; 6] = [
        ToolSubstrate::Carbide,
        ToolSubstrate::CarbideUltraFine,
        ToolSubstrate::Hss,
        ToolSubstrate::HssCobalt,
        ToolSubstrate::Cbn,
        ToolSubstrate::Pcd,
    ];

    pub fn key(&self) -> &'static str {
        match self {
            Self::Carbide => "carbide",
            Self::CarbideUltraFine => "carbide_uf",
            Self::Hss => "hss",
            Self::HssCobalt => "hss_co",
            Self::Cbn => "cbn",
            Self::Pcd => "pcd",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_lowercase().replace(['-', ' '], "_").as_str() {
            "carbide" | "초경" | "wc" | "fine" => Some(Self::Carbide),
            "carbide_uf" | "ultrafine" | "ultra_fine" | "초미립" | "초미립_초경" => Some(Self::CarbideUltraFine),
            "hss" | "m2" | "고속도강" => Some(Self::Hss),
            "hss_co" | "hssco" | "hss_e" | "m42" | "코발트" | "코발트_hss" => Some(Self::HssCobalt),
            "cbn" => Some(Self::Cbn),
            "pcd" | "diamond_tip" => Some(Self::Pcd),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Carbide => "초경 (미립, WC-10Co)",
            Self::CarbideUltraFine => "초경 (초미립, WC-12Co)",
            Self::Hss => "고속도강 HSS (M2)",
            Self::HssCobalt => "코발트 HSS (M42)",
            Self::Cbn => "CBN",
            Self::Pcd => "PCD (소결 다이아몬드)",
        }
    }

    pub fn base(&self) -> crate::material::ToolBaseMaterial {
        use crate::material::{CarbideGrade, HSSGrade, ToolBaseMaterial};
        match self {
            Self::Carbide => ToolBaseMaterial::SolidCarbide(CarbideGrade::FineGrain { grain_size_um: 0.8, cobalt_percent: 10.0 }),
            Self::CarbideUltraFine => ToolBaseMaterial::SolidCarbide(CarbideGrade::UltraFine { grain_size_um: 0.4, cobalt_percent: 12.0 }),
            Self::Hss => ToolBaseMaterial::HSS(HSSGrade::M2),
            Self::HssCobalt => ToolBaseMaterial::HSS(HSSGrade::M42),
            Self::Cbn => ToolBaseMaterial::CBN,
            Self::Pcd => ToolBaseMaterial::PCD,
        }
    }

    pub fn max_temp_c(&self) -> f64 {
        match self {
            Self::Hss => 550.0,
            Self::HssCobalt => 600.0,
            Self::Pcd => 700.0,
            Self::Cbn => 1300.0,
            Self::Carbide | Self::CarbideUltraFine => 1000.0,
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
    #[serde(default)]
    pub substrate: ToolSubstrate,
    #[serde(default)]
    pub rake_deg: Option<f64>,
    #[serde(default)]
    pub clearance_deg: Option<f64>,
    #[serde(default)]
    pub edge_radius_um: Option<f64>,
    #[serde(default)]
    pub neck_diameter_mm: Option<f64>,
    #[serde(default)]
    pub reach_mm: Option<f64>,
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
            substrate: ToolSubstrate::Carbide,
            rake_deg: None,
            clearance_deg: None,
            edge_radius_um: None,
            neck_diameter_mm: None,
            reach_mm: None,
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

    pub fn dynamic_key(&self) -> String {
        let mut key = format!(
            "D{:.2}Z{}-{}-L{:.1}-O{:.1}-S{:.2}-H{:.0}",
            self.diameter_mm,
            self.flute_count,
            self.nose.key(),
            self.loc_mm,
            self.oal_mm,
            self.shank_diameter_mm,
            self.helix_angle_deg
        );
        if let Some((d, r)) = self.neck() {
            key.push_str(&format!("-N{:.2}x{:.1}", d, r));
        }
        if self.substrate != ToolSubstrate::Carbide {
            key.push_str(&format!("-{}", self.substrate.key()));
        }
        key
    }

    pub fn signature(&self) -> String {
        let mut sig = format!(
            "D{:.1}Z{}-{}-{}",
            self.diameter_mm,
            self.flute_count,
            self.nose.key(),
            self.coating_name.clone().unwrap_or_else(|| "bare".into()).to_lowercase().replace(' ', "_")
        );
        if self.substrate != ToolSubstrate::Carbide {
            sig.push_str(&format!("-{}", self.substrate.key()));
        }
        sig
    }

    pub fn effective_stickout_mm(&self) -> f64 {
        let reach = self.neck().map(|(_, r)| r).unwrap_or(self.loc_mm);
        if let Some(s) = self.stickout_mm.filter(|v| *v > 0.0) {
            return s.max(reach);
        }
        let grip = (self.oal_mm - reach - 0.2 * self.diameter_mm).min(4.0 * self.shank_diameter_mm);
        if grip > 0.0 {
            (self.oal_mm - grip).max(reach + 0.5 * self.diameter_mm)
        } else {
            reach + 10.0
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
        if let Some(r) = self.rake_deg {
            if !(r.is_finite() && (-15.0..=30.0).contains(&r)) {
                return Err("경사각(레이크)은 -15~30° 범위여야 합니다".into());
            }
        }
        if let Some(c) = self.clearance_deg {
            if !(c.is_finite() && (3.0..=25.0).contains(&c)) {
                return Err("여유각은 3~25° 범위여야 합니다".into());
            }
        }
        if let Some(e) = self.edge_radius_um {
            if !(e.is_finite() && (0.5..=60.0).contains(&e)) {
                return Err("날끝 호닝 반경은 0.5~60 µm 범위여야 합니다".into());
            }
        }
        match (self.neck_diameter_mm, self.reach_mm) {
            (None, None) => {}
            (Some(dn), Some(reach)) => {
                if !(dn.is_finite() && dn > 0.3 * self.diameter_mm && dn < self.diameter_mm) {
                    return Err("넥 직경은 공구 직경의 30~100% 사이여야 합니다".into());
                }
                if !(reach.is_finite() && reach > self.loc_mm && reach < self.oal_mm) {
                    return Err("넥 유효장(날끝~넥 끝)은 유효 절삭 길이보다 길고 전체 길이보다 짧아야 합니다".into());
                }
                if let Some(s) = self.stickout_mm {
                    if s < reach {
                        return Err("넥 공구는 돌출 길이가 넥 유효장 이상이어야 합니다".into());
                    }
                }
            }
            _ => return Err("넥 직경과 넥 유효장은 함께 입력하세요".into()),
        }
        Ok(())
    }

    pub fn neck(&self) -> Option<(f64, f64)> {
        match (self.neck_diameter_mm, self.reach_mm) {
            (Some(d), Some(r)) if d > 0.0 && r > self.loc_mm => Some((d, r)),
            _ => None,
        }
    }

    pub fn shape_label(&self) -> String {
        let mut parts = vec![self.nose.label()];
        if let Some((d, r)) = self.neck() {
            parts.push(format!("롱넥 Ø{:.2}×{:.0}", d, r));
        }
        if self.variable_pitch {
            parts.push("부등분할".into());
        }
        parts.join(" · ")
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
            substrate: ToolSubstrate::Carbide,
            rake_deg: None,
            clearance_deg: None,
            edge_radius_um: None,
            neck_diameter_mm: None,
            reach_mm: None,
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
            substrate: ToolSubstrate::Carbide,
            rake_deg: None,
            clearance_deg: None,
            edge_radius_um: None,
            neck_diameter_mm: None,
            reach_mm: None,
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
            substrate: ToolSubstrate::Carbide,
            rake_deg: None,
            clearance_deg: None,
            edge_radius_um: None,
            neck_diameter_mm: None,
            reach_mm: None,
        }
    }

    pub fn default_8mm_3flute_alcrn() -> Self {
        Self {
            name: "Ø8 3날 AlCrN (스테인리스·티타늄)".into(),
            model: "BUILTIN-AC8F3".into(),
            diameter_mm: 8.0,
            flute_count: 3,
            loc_mm: 20.0,
            oal_mm: 63.0,
            shank_diameter_mm: 8.0,
            helix_angle_deg: 38.0,
            coating_name: Some("AlCrN".into()),
            is_high_end: true,
            nose: ToolNose::CornerRadius { radius_mm: 0.3 },
            stickout_mm: None,
            runout_um: None,
            variable_pitch: true,
            substrate: ToolSubstrate::CarbideUltraFine,
            rake_deg: Some(10.0),
            clearance_deg: Some(9.0),
            edge_radius_um: Some(8.0),
            neck_diameter_mm: None,
            reach_mm: None,
        }
    }

    pub fn default_6mm_ball_long_neck() -> Self {
        Self {
            name: "Ø6 2날 볼 롱넥 (고경도 금형)".into(),
            model: "BUILTIN-HB6N18".into(),
            diameter_mm: 6.0,
            flute_count: 2,
            loc_mm: 6.0,
            oal_mm: 70.0,
            shank_diameter_mm: 6.0,
            helix_angle_deg: 30.0,
            coating_name: Some("TiSiN".into()),
            is_high_end: true,
            nose: ToolNose::Ball,
            stickout_mm: None,
            runout_um: Some(2.0),
            variable_pitch: false,
            substrate: ToolSubstrate::CarbideUltraFine,
            rake_deg: Some(-5.0),
            clearance_deg: Some(12.0),
            edge_radius_um: Some(10.0),
            neck_diameter_mm: Some(5.7),
            reach_mm: Some(18.0),
        }
    }

    pub fn default_10mm_diamond_cfrp() -> Self {
        Self {
            name: "Ø10 6날 다이아몬드 코팅 (CFRP 트리밍)".into(),
            model: "BUILTIN-DC10F6".into(),
            diameter_mm: 10.0,
            flute_count: 6,
            loc_mm: 22.0,
            oal_mm: 75.0,
            shank_diameter_mm: 10.0,
            helix_angle_deg: 15.0,
            coating_name: Some("Diamond".into()),
            is_high_end: true,
            nose: ToolNose::Square,
            stickout_mm: None,
            runout_um: None,
            variable_pitch: false,
            substrate: ToolSubstrate::Carbide,
            rake_deg: Some(8.0),
            clearance_deg: Some(15.0),
            edge_radius_um: Some(12.0),
            neck_diameter_mm: None,
            reach_mm: None,
        }
    }

    pub fn default_12mm_5flute_superalloy() -> Self {
        Self {
            name: "Ø12 5날 AlTiN (내열합금 황삭)".into(),
            model: "BUILTIN-AT12F5".into(),
            diameter_mm: 12.0,
            flute_count: 5,
            loc_mm: 26.0,
            oal_mm: 83.0,
            shank_diameter_mm: 12.0,
            helix_angle_deg: 42.0,
            coating_name: Some("AlTiN".into()),
            is_high_end: true,
            nose: ToolNose::CornerRadius { radius_mm: 1.0 },
            stickout_mm: None,
            runout_um: None,
            variable_pitch: true,
            substrate: ToolSubstrate::CarbideUltraFine,
            rake_deg: Some(8.0),
            clearance_deg: Some(10.0),
            edge_radius_um: Some(15.0),
            neck_diameter_mm: None,
            reach_mm: None,
        }
    }

    pub fn default_8mm_3flute_dlc() -> Self {
        Self {
            name: "Ø8 3날 DLC (알루미늄 정삭)".into(),
            model: "BUILTIN-DL8F3".into(),
            diameter_mm: 8.0,
            flute_count: 3,
            loc_mm: 20.0,
            oal_mm: 63.0,
            shank_diameter_mm: 8.0,
            helix_angle_deg: 45.0,
            coating_name: Some("DLC".into()),
            is_high_end: true,
            nose: ToolNose::Square,
            stickout_mm: None,
            runout_um: None,
            variable_pitch: false,
            substrate: ToolSubstrate::Carbide,
            rake_deg: Some(16.0),
            clearance_deg: Some(12.0),
            edge_radius_um: Some(4.0),
            neck_diameter_mm: None,
            reach_mm: None,
        }
    }

    pub fn builtin_tools() -> Vec<Self> {
        vec![
            Self::default_10mm_4flute(),
            Self::default_6mm_2flute_aluminum(),
            Self::default_12mm_ball_nose(),
            Self::default_8mm_3flute_alcrn(),
            Self::default_6mm_ball_long_neck(),
            Self::default_10mm_diamond_cfrp(),
            Self::default_12mm_5flute_superalloy(),
            Self::default_8mm_3flute_dlc(),
        ]
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
    #[serde(default)]
    pub purpose: Option<String>,
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
            purpose: Some("roughing".into()),
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
            purpose: Some("finishing".into()),
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
            purpose: Some("roughing".into()),
        }
    }

    pub fn from_parts(
        name: &str,
        description: &str,
        endmill: EndMillMockupSetting,
        workpiece: WorkpieceSetup,
        coolant: CoolantConfig,
        purpose: &str,
    ) -> Self {
        let material = workpiece.effective_material();
        let mut conditions = crate::cutting::CuttingCalculator::recommend_conditions(
            &material,
            endmill.diameter_mm,
            endmill.flute_count,
            endmill.is_high_end,
        )
        .unwrap_or(CuttingConditions {
            cutting_speed_m_min: 100.0,
            feed_rate_mm_min: 800.0,
            feed_per_tooth_mm: 0.05,
            axial_doc_mm: endmill.diameter_mm,
            radial_doc_mm: 0.3 * endmill.diameter_mm,
            spindle_rpm: 3000,
            coolant: crate::cutting::CoolantType::Flood,
        });
        conditions.coolant = match coolant.method {
            CoolantMethod::AirBlast => crate::cutting::CoolantType::AirBlast,
            CoolantMethod::Flood => crate::cutting::CoolantType::Flood,
            CoolantMethod::Mist => crate::cutting::CoolantType::Mist,
            CoolantMethod::ThroughTool => crate::cutting::CoolantType::ThroughTool,
            CoolantMethod::Dry => crate::cutting::CoolantType::Dry,
        };
        conditions.axial_doc_mm = conditions.axial_doc_mm.min(0.9 * endmill.loc_mm);
        if purpose == "finishing" {
            conditions.radial_doc_mm = conditions.radial_doc_mm.min((0.1 * endmill.diameter_mm).max(workpiece.stock_allowance_mm));
        }
        Self {
            name: name.into(),
            description: description.into(),
            endmill_setting: endmill,
            conditions,
            workpiece_setup: workpiece,
            coolant_config: coolant,
            purpose: Some(purpose.into()),
        }
    }

    pub fn default_stainless_finishing() -> Self {
        Self::from_parts(
            "스테인리스 정삭 프리셋",
            "SUS304 벽면 정삭 · 가공경화를 피하는 칩두께 · 플러드",
            EndMillMockupSetting::default_8mm_3flute_alcrn(),
            WorkpieceSetup::default_stainless_plate(),
            CoolantConfig::flood(),
            "finishing",
        )
    }

    pub fn default_superalloy_roughing() -> Self {
        Self::from_parts(
            "인코넬 황삭 프리셋",
            "Inconel 718 황삭 · 5날 부등분할 · 내부 급유",
            EndMillMockupSetting::default_12mm_5flute_superalloy(),
            WorkpieceSetup::default_inconel_block(),
            CoolantConfig::through_tool(),
            "roughing",
        )
    }

    pub fn default_hardened_finishing() -> Self {
        Self::from_parts(
            "금형강 고경도 정삭 프리셋",
            "SKD11 HRC58 곡면 정삭 · 롱넥 볼 · 에어 블로우 (열충격 회피)",
            EndMillMockupSetting::default_6mm_ball_long_neck(),
            WorkpieceSetup::default_hardened_block(),
            CoolantConfig::air_blast(),
            "finishing",
        )
    }

    pub fn default_cfrp_trimming() -> Self {
        Self::from_parts(
            "CFRP 트리밍 프리셋",
            "CFRP 외곽 트리밍 · 다이아몬드 코팅 · 에어 블로우 + 집진",
            EndMillMockupSetting::default_10mm_diamond_cfrp(),
            WorkpieceSetup::default_cfrp_panel(),
            CoolantConfig::air_blast(),
            "roughing",
        )
    }

    pub fn default_aluminum_finishing() -> Self {
        Self::from_parts(
            "알루미늄 정삭 프리셋",
            "알루미늄 벽면 정삭 · DLC 3날 고경사 · MQL",
            EndMillMockupSetting::default_8mm_3flute_dlc(),
            WorkpieceSetup::default_aluminum_plate(),
            CoolantConfig::mist(),
            "finishing",
        )
    }

    pub fn builtin() -> Vec<Self> {
        vec![
            Self::default_aluminum_roughing(),
            Self::default_titanium_finishing(),
            Self::default_steel_general(),
            Self::default_stainless_finishing(),
            Self::default_superalloy_roughing(),
            Self::default_hardened_finishing(),
            Self::default_cfrp_trimming(),
            Self::default_aluminum_finishing(),
        ]
    }

    pub fn normalize_legacy(&mut self) {
        self.coolant_config.normalize_legacy();
        if self.purpose.is_none() {
            self.purpose = purpose_from_name(&self.name);
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
    #[serde(default = "default_damping_ratio")]
    pub damping_ratio: f64,
    #[serde(default = "default_holder_mass_kg")]
    pub holder_mass_kg: f64,
    #[serde(default)]
    pub tip_fn_hz: Option<f64>,
    #[serde(default)]
    pub tip_stiffness_n_per_um: Option<f64>,
    #[serde(default)]
    pub tip_frf_stickout_mm: Option<f64>,
    #[serde(default)]
    pub tip_frf_tool: Option<String>,
    #[serde(default)]
    pub environment: ShopEnvironment,
}

fn default_damping_ratio() -> f64 {
    0.03
}

fn default_holder_mass_kg() -> f64 {
    0.3
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
            damping_ratio: default_damping_ratio(),
            holder_mass_kg: default_holder_mass_kg(),
            tip_fn_hz: None,
            tip_stiffness_n_per_um: None,
            tip_frf_stickout_mm: None,
            tip_frf_tool: None,
            environment: ShopEnvironment::default(),
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
    #[serde(default)]
    pub purpose: Option<String>,
}

pub fn purpose_from_name(name: &str) -> Option<String> {
    let n = name.to_lowercase();
    if n.contains("정삭") || n.contains("finishing") {
        Some("finishing".into())
    } else if n.contains("황삭") || n.contains("roughing") {
        Some("roughing".into())
    } else {
        None
    }
}

impl MachiningProfile {
    pub fn normalize_legacy(&mut self) {
        self.coolant_config.normalize_legacy();
        if self.purpose.is_none() {
            self.purpose = purpose_from_name(&self.name);
        }
    }

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
            purpose: preset.purpose.clone().or_else(|| purpose_from_name(&preset.name)),
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
        let presets = MachiningPreset::builtin();
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
            purpose: None,
        };
        self.upsert_profile(profile.clone());
        Ok(self.profiles.iter().find(|p| p.name == profile.name).unwrap())
    }
}
