use serde::{Deserialize, Serialize};
use crate::cutting::WorkpieceMaterial;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum StockShape {
    Rectangular,
    Cylindrical { diameter_mm: f64 },
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClampingMethod {
    Vise,
    VacuumChuck,
    MagneticChuck,
    FixturePlate,
    SoftJaw,
}

impl ClampingMethod {
    pub fn stiffness_n_per_um(&self) -> f64 {
        match self {
            ClampingMethod::Vise => 80.0,
            ClampingMethod::FixturePlate => 120.0,
            ClampingMethod::SoftJaw => 60.0,
            ClampingMethod::MagneticChuck => 40.0,
            ClampingMethod::VacuumChuck => 20.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkpieceSetup {
    pub name: String,
    pub material: WorkpieceMaterial,
    pub shape: StockShape,
    pub width_mm: f64,
    pub height_mm: f64,
    pub thickness_mm: f64,
    pub clamping: ClampingMethod,
    pub stock_allowance_mm: f64,
    pub zero_point: (f64, f64),
    pub surface_roughness_target_ra: f64,
    pub hardness_hrc: Option<u8>,
    pub grain_direction_deg: f64,
    pub pre_machined: bool,
    #[serde(default)]
    pub tolerance_mm: Option<f64>,
}

impl WorkpieceSetup {
    pub fn default_aluminum_plate() -> Self {
        Self {
            name: "알루미늄 판재 (기본)".into(),
            material: WorkpieceMaterial::Aluminum,
            shape: StockShape::Rectangular,
            width_mm: 100.0,
            height_mm: 80.0,
            thickness_mm: 20.0,
            clamping: ClampingMethod::Vise,
            stock_allowance_mm: 0.5,
            zero_point: (0.0, 0.0),
            surface_roughness_target_ra: 1.6,
            hardness_hrc: None,
            grain_direction_deg: 0.0,
            pre_machined: false,
            tolerance_mm: None,
        }
    }

    pub fn default_steel_block() -> Self {
        Self {
            name: "탄소강 블록 (기본)".into(),
            material: WorkpieceMaterial::CarbonSteel,
            shape: StockShape::Rectangular,
            width_mm: 150.0,
            height_mm: 100.0,
            thickness_mm: 30.0,
            clamping: ClampingMethod::Vise,
            stock_allowance_mm: 1.0,
            zero_point: (0.0, 0.0),
            surface_roughness_target_ra: 3.2,
            hardness_hrc: Some(25),
            grain_direction_deg: 0.0,
            pre_machined: false,
            tolerance_mm: None,
        }
    }

    pub fn default_titanium_plate() -> Self {
        Self {
            name: "티타늄 판재 (기본)".into(),
            material: WorkpieceMaterial::Titanium,
            shape: StockShape::Rectangular,
            width_mm: 80.0,
            height_mm: 60.0,
            thickness_mm: 15.0,
            clamping: ClampingMethod::FixturePlate,
            stock_allowance_mm: 0.3,
            zero_point: (0.0, 0.0),
            surface_roughness_target_ra: 0.8,
            hardness_hrc: Some(36),
            grain_direction_deg: 0.0,
            pre_machined: false,
            tolerance_mm: None,
        }
    }

    pub fn default_cylindrical_steel() -> Self {
        Self {
            name: "원형 탄소강 (기본)".into(),
            material: WorkpieceMaterial::CarbonSteel,
            shape: StockShape::Cylindrical { diameter_mm: 50.0 },
            width_mm: 50.0,
            height_mm: 50.0,
            thickness_mm: 100.0,
            clamping: ClampingMethod::Vise,
            stock_allowance_mm: 1.0,
            zero_point: (25.0, 25.0),
            surface_roughness_target_ra: 3.2,
            hardness_hrc: Some(25),
            grain_direction_deg: 0.0,
            pre_machined: false,
            tolerance_mm: None,
        }
    }

    pub fn default_inconel_block() -> Self {
        Self {
            name: "인코넬 블록 (기본)".into(),
            material: WorkpieceMaterial::Inconel,
            shape: StockShape::Rectangular,
            width_mm: 60.0,
            height_mm: 60.0,
            thickness_mm: 25.0,
            clamping: ClampingMethod::FixturePlate,
            stock_allowance_mm: 0.5,
            zero_point: (0.0, 0.0),
            surface_roughness_target_ra: 0.8,
            hardness_hrc: Some(40),
            grain_direction_deg: 0.0,
            pre_machined: false,
            tolerance_mm: None,
        }
    }

    pub fn effective_material(&self) -> WorkpieceMaterial {
        match (&self.material, self.hardness_hrc) {
            (WorkpieceMaterial::AlloySteel { .. }, Some(h)) => WorkpieceMaterial::AlloySteel { hardness_hrc: h },
            (m, _) => m.clone(),
        }
    }

    pub fn effective_tolerance_mm(&self) -> f64 {
        if let Some(t) = self.tolerance_mm.filter(|t| *t > 0.0) {
            return t;
        }
        let ra = self.surface_roughness_target_ra;
        if ra <= 0.8 {
            0.01
        } else if ra <= 1.6 {
            0.02
        } else if ra <= 3.2 {
            0.05
        } else {
            0.1
        }
    }

    pub fn surface_area_m2(&self) -> f64 {
        match &self.shape {
            StockShape::Cylindrical { diameter_mm } => {
                let r = diameter_mm / 2.0;
                (2.0 * std::f64::consts::PI * r * r + 2.0 * std::f64::consts::PI * r * self.thickness_mm) / 1.0e6
            }
            _ => {
                2.0 * (self.width_mm * self.height_mm
                    + self.width_mm * self.thickness_mm
                    + self.height_mm * self.thickness_mm)
                    / 1.0e6
            }
        }
    }

    pub fn volume_cm3(&self) -> f64 {
        match &self.shape {
            StockShape::Rectangular => {
                (self.width_mm * self.height_mm * self.thickness_mm) / 1000.0
            }
            StockShape::Cylindrical { diameter_mm } => {
                let r = diameter_mm / 2.0;
                (std::f64::consts::PI * r * r * self.thickness_mm) / 1000.0
            }
            StockShape::Custom => 0.0,
        }
    }

    pub fn material_label(&self) -> String {
        match &self.effective_material() {
            WorkpieceMaterial::Aluminum => "알루미늄 합금".into(),
            WorkpieceMaterial::CarbonSteel => "탄소강 (S45C)".into(),
            WorkpieceMaterial::AlloySteel { hardness_hrc } => format!("합금강 (HRC {})", hardness_hrc),
            WorkpieceMaterial::StainlessSteel => "스테인리스강 (SUS304)".into(),
            WorkpieceMaterial::Titanium => "티타늄 (Ti-6Al-4V)".into(),
            WorkpieceMaterial::Inconel => "인코넬 718".into(),
            WorkpieceMaterial::SuperAlloy => "수퍼알로이".into(),
            WorkpieceMaterial::CFRP => "CFRP".into(),
        }
    }
}