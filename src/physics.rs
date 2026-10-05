use crate::cutting::{CoolantType, CuttingConditions, WorkpieceMaterial};
use crate::material::{CarbideGrade, ToolBaseMaterial};
use crate::profile::{CoolantConfig, CoolantMethod, EndMillMockupSetting, MachineLimits, MachiningProfile, ToolNose};
use serde::{Deserialize, Serialize};
use std::f64::consts::PI;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkpieceProps {
    pub key: String,
    pub label: String,
    pub density: f64,
    pub specific_heat: f64,
    pub conductivity: f64,
    pub expansion: f64,
    pub elastic_gpa: f64,
    pub hardness_hrc: f64,
    pub kc11: f64,
    pub mc: f64,
    pub rake_deg: f64,
    pub adhesion: f64,
    pub wear_activation_k: f64,
    pub thermal_factor: f64,
    pub melt_c: f64,
    pub temp_limit_c: Option<f64>,
    pub bue_range_c: Option<(f64, f64)>,
    pub ref_vc: f64,
    pub fz_coeff: f64,
    pub ap_ratio: f64,
    pub ae_ratio: f64,
    pub edge_radius_um: f64,
    pub ref_life_min: f64,
}

fn interp(points: &[(f64, f64)], x: f64) -> f64 {
    if x <= points[0].0 {
        return points[0].1;
    }
    for w in points.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if x <= x1 {
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0);
        }
    }
    points[points.len() - 1].1
}

impl WorkpieceProps {
    pub fn of(m: &WorkpieceMaterial) -> Self {
        let base = |key: &str, label: &str| WorkpieceProps {
            key: key.into(),
            label: label.into(),
            density: 7850.0,
            specific_heat: 486.0,
            conductivity: 49.8,
            expansion: 11.5e-6,
            elastic_gpa: 205.0,
            hardness_hrc: 15.0,
            kc11: 1600.0,
            mc: 0.25,
            rake_deg: 10.0,
            adhesion: 1.0,
            wear_activation_k: 12000.0,
            thermal_factor: 1.0,
            melt_c: 1450.0,
            temp_limit_c: None,
            bue_range_c: Some((250.0, 450.0)),
            ref_vc: m.reference_cutting_speed(),
            fz_coeff: 0.005,
            ap_ratio: 1.0,
            ae_ratio: 0.3,
            edge_radius_um: 12.0,
            ref_life_min: 60.0,
        };
        match m {
            WorkpieceMaterial::Aluminum => WorkpieceProps {
                density: 2700.0,
                specific_heat: 896.0,
                conductivity: 167.0,
                expansion: 23.6e-6,
                elastic_gpa: 69.0,
                hardness_hrc: 0.0,
                kc11: 700.0,
                mc: 0.25,
                rake_deg: 18.0,
                adhesion: 1.25,
                wear_activation_k: 6000.0,
                melt_c: 650.0,
                bue_range_c: Some((150.0, 300.0)),
                fz_coeff: 0.010,
                ae_ratio: 0.5,
                edge_radius_um: 4.0,
                ref_life_min: 240.0,
                ..base("aluminum", "알루미늄 합금")
            },
            WorkpieceMaterial::CarbonSteel => WorkpieceProps {
                hardness_hrc: 15.0,
                ..base("carbon_steel", "탄소강 (S45C)")
            },
            WorkpieceMaterial::AlloySteel { hardness_hrc } => {
                let h = *hardness_hrc as f64;
                let kc = interp(
                    &[(20.0, 1950.0), (30.0, 2100.0), (40.0, 2500.0), (50.0, 3100.0), (60.0, 3800.0), (65.0, 4200.0)],
                    h,
                );
                let fz = if h <= 30.0 {
                    0.005
                } else if h <= 45.0 {
                    0.0045
                } else if h <= 55.0 {
                    0.0035
                } else {
                    0.0025
                };
                WorkpieceProps {
                    density: 7800.0,
                    specific_heat: 460.0,
                    conductivity: 30.0,
                    expansion: 11.5e-6,
                    elastic_gpa: 210.0,
                    hardness_hrc: h,
                    kc11: kc,
                    mc: 0.25,
                    rake_deg: (8.0 - 0.5 * (h - 40.0).max(0.0)).clamp(-6.0, 8.0),
                    adhesion: 0.95,
                    thermal_factor: (1.0 - 0.015 * (h - 25.0).max(0.0)).clamp(0.5, 1.0),
                    ref_life_min: if h > 45.0 { 35.0 } else { 60.0 },
                    bue_range_c: if h > 40.0 { None } else { Some((300.0, 480.0)) },
                    ref_vc: if h <= 30.0 {
                        150.0
                    } else if h <= 45.0 {
                        110.0
                    } else if h <= 55.0 {
                        90.0
                    } else {
                        70.0
                    },
                    fz_coeff: fz,
                    ae_ratio: if h > 45.0 { 0.1 } else { 0.3 },
                    edge_radius_um: if h > 45.0 { 20.0 } else { 12.0 },
                    ..base(&format!("alloy_steel_hrc{}", hardness_hrc), &format!("합금강/금형강 (HRC {})", hardness_hrc))
                }
            }
            WorkpieceMaterial::StainlessSteel => WorkpieceProps {
                density: 8000.0,
                specific_heat: 500.0,
                conductivity: 16.2,
                expansion: 17.3e-6,
                elastic_gpa: 193.0,
                hardness_hrc: 20.0,
                kc11: 2000.0,
                mc: 0.21,
                rake_deg: 12.0,
                adhesion: 1.15,
                thermal_factor: 0.85,
                bue_range_c: Some((300.0, 500.0)),
                fz_coeff: 0.004,
                ae_ratio: 0.25,
                edge_radius_um: 10.0,
                ref_life_min: 45.0,
                ..base("stainless", "스테인리스강 (SUS304)")
            },
            WorkpieceMaterial::Titanium => WorkpieceProps {
                density: 4430.0,
                specific_heat: 526.0,
                conductivity: 6.7,
                expansion: 8.6e-6,
                elastic_gpa: 114.0,
                hardness_hrc: 36.0,
                kc11: 1400.0,
                mc: 0.23,
                rake_deg: 10.0,
                adhesion: 1.1,
                wear_activation_k: 14000.0,
                thermal_factor: 0.7,
                melt_c: 1650.0,
                bue_range_c: None,
                fz_coeff: 0.004,
                ae_ratio: 0.2,
                edge_radius_um: 10.0,
                ref_life_min: 35.0,
                ..base("titanium", "티타늄 (Ti-6Al-4V)")
            },
            WorkpieceMaterial::Inconel => WorkpieceProps {
                density: 8190.0,
                specific_heat: 435.0,
                conductivity: 11.4,
                expansion: 13.0e-6,
                elastic_gpa: 200.0,
                hardness_hrc: 40.0,
                kc11: 2900.0,
                mc: 0.25,
                rake_deg: 8.0,
                adhesion: 1.1,
                wear_activation_k: 14000.0,
                thermal_factor: 0.7,
                melt_c: 1300.0,
                bue_range_c: None,
                fz_coeff: 0.003,
                ae_ratio: 0.1,
                edge_radius_um: 10.0,
                ref_life_min: 25.0,
                ..base("inconel", "인코넬 718")
            },
            WorkpieceMaterial::SuperAlloy => WorkpieceProps {
                density: 8250.0,
                specific_heat: 450.0,
                conductivity: 11.0,
                expansion: 12.5e-6,
                elastic_gpa: 210.0,
                hardness_hrc: 42.0,
                kc11: 3100.0,
                mc: 0.25,
                rake_deg: 6.0,
                adhesion: 1.1,
                wear_activation_k: 14000.0,
                thermal_factor: 0.7,
                melt_c: 1300.0,
                bue_range_c: None,
                fz_coeff: 0.0025,
                ae_ratio: 0.1,
                edge_radius_um: 10.0,
                ref_life_min: 20.0,
                ..base("superalloy", "수퍼알로이")
            },
            WorkpieceMaterial::CFRP => WorkpieceProps {
                density: 1550.0,
                specific_heat: 1000.0,
                conductivity: 5.0,
                expansion: 1.0e-6,
                elastic_gpa: 70.0,
                hardness_hrc: 0.0,
                kc11: 600.0,
                mc: 0.20,
                rake_deg: 10.0,
                adhesion: 0.6,
                wear_activation_k: 4000.0,
                thermal_factor: 0.5,
                melt_c: 400.0,
                temp_limit_c: Some(180.0),
                bue_range_c: None,
                fz_coeff: 0.006,
                ae_ratio: 0.5,
                edge_radius_um: 3.0,
                ref_life_min: 60.0,
                ..base("cfrp", "CFRP")
            },
        }
    }

    pub fn rho_c(&self) -> f64 {
        self.density * self.specific_heat
    }

    pub fn reference_coolant(&self) -> CoolantConfig {
        if self.key == "aluminum" {
            CoolantConfig::flood()
        } else if self.key == "cfrp" || self.hardness_hrc >= 45.0 {
            CoolantConfig::air_blast()
        } else if self.key == "titanium" || self.key == "inconel" || self.key == "superalloy" {
            CoolantConfig::through_tool()
        } else {
            CoolantConfig::flood()
        }
    }

    pub fn diffusivity(&self) -> f64 {
        self.conductivity / self.rho_c()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoatingTribology {
    pub name: String,
    pub family: String,
    pub friction: f64,
    pub max_temp_c: f64,
    pub hardness_hv: f64,
    pub wear_factor: f64,
    pub ferrous_ok: bool,
}

pub fn tribology_from_name(name: Option<&str>) -> CoatingTribology {
    let raw = name.unwrap_or("").trim().to_string();
    let n = raw.to_lowercase().replace([' ', '-', '_'], "");
    let make = |family: &str, friction: f64, max_temp_c: f64, hardness_hv: f64, wear_factor: f64, ferrous_ok: bool| CoatingTribology {
        name: if raw.is_empty() { "무코팅".into() } else { raw.clone() },
        family: family.into(),
        friction,
        max_temp_c,
        hardness_hv,
        wear_factor,
        ferrous_ok,
    };
    if n.is_empty() || n.contains("uncoated") || n.contains("bare") || n.contains("무코팅") || n == "none" {
        return make("uncoated", 0.60, 800.0, 1600.0, 1.00, true);
    }
    if n.contains("naco") {
        return make("nACo", 0.45, 1200.0, 4000.0, 0.32, true);
    }
    if n.contains("tisin") || n.contains("sisn") {
        return make("TiSiN", 0.45, 1100.0, 3800.0, 0.35, true);
    }
    if n.contains("alcrn") || n.contains("crall") {
        return make("AlCrN", 0.40, 1100.0, 3200.0, 0.38, true);
    }
    if n.contains("altin") {
        return make("AlTiN", 0.50, 900.0, 3500.0, 0.40, true);
    }
    if n.contains("tialn") {
        return make("TiAlN", 0.45, 800.0, 3300.0, 0.45, true);
    }
    if n.contains("ticn") {
        return make("TiCN", 0.40, 400.0, 3000.0, 0.60, true);
    }
    if n.contains("dlc") || n.contains("diamondlike") {
        return make("DLC", 0.12, 400.0, 2500.0, 0.50, false);
    }
    if n.contains("diamond") || n.contains("cvd") || n.contains("pcd") {
        return make("Diamond", 0.10, 700.0, 9000.0, 0.10, false);
    }
    if n.contains("zrn") {
        return make("ZrN", 0.35, 550.0, 2500.0, 0.60, false);
    }
    if n.contains("crn") {
        return make("CrN", 0.40, 700.0, 1800.0, 0.65, true);
    }
    if n.contains("tin") {
        return make("TiN", 0.50, 600.0, 2300.0, 0.70, true);
    }
    if n.contains("hipims") {
        return make("HiPIMS", 0.45, 1000.0, 3600.0, 0.38, true);
    }
    make("unknown", 0.50, 800.0, 3000.0, 0.50, true)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolGeometry {
    pub diameter_mm: f64,
    pub flutes: u32,
    pub helix_deg: f64,
    pub loc_mm: f64,
    pub oal_mm: f64,
    pub shank_mm: f64,
    pub nose: ToolNose,
    pub stickout_mm: f64,
    pub rake_deg: f64,
    pub clearance_deg: f64,
    pub edge_radius_um: f64,
    pub core_ratio: f64,
    pub runout_um: f64,
    pub variable_pitch: bool,
    pub elastic_gpa: f64,
    pub tool_conductivity: f64,
    pub tool_expansion: f64,
    pub tool_rho_c: f64,
    pub base_wear_factor: f64,
    pub coating: CoatingTribology,
    pub holder_stiffness_n_per_um: f64,
}

impl ToolGeometry {
    pub fn from_setting(s: &EndMillMockupSetting, wp: &WorkpieceProps, machine: &MachineLimits, base: Option<&ToolBaseMaterial>) -> Self {
        let z = s.flute_count.max(1) as u32;
        let core = if z <= 2 {
            0.75
        } else if z == 3 {
            0.78
        } else {
            0.80
        };
        let (e, k, alpha, rho_c, wf) = match base {
            Some(ToolBaseMaterial::HSS(_)) => (210.0, 25.0, 11.0e-6, 8100.0 * 460.0, 3.0),
            Some(ToolBaseMaterial::CBN) => (680.0, 100.0, 4.7e-6, 3480.0 * 800.0, if wp.hardness_hrc >= 45.0 { 0.3 } else { 1.5 }),
            Some(ToolBaseMaterial::PCD) => (900.0, 500.0, 3.0e-6, 3500.0 * 510.0, if wp.key == "aluminum" || wp.key == "cfrp" { 0.1 } else { 10.0 }),
            Some(ToolBaseMaterial::SolidCarbide(CarbideGrade::UltraFine { .. })) => (620.0, 75.0, 5.2e-6, 14500.0 * 220.0, 0.9),
            _ => (600.0, 80.0, 5.5e-6, 14500.0 * 220.0, 1.0),
        };
        Self {
            diameter_mm: s.diameter_mm,
            flutes: z,
            helix_deg: s.helix_angle_deg,
            loc_mm: s.loc_mm,
            oal_mm: s.oal_mm,
            shank_mm: s.shank_diameter_mm,
            nose: s.nose,
            stickout_mm: s.effective_stickout_mm(),
            rake_deg: wp.rake_deg,
            clearance_deg: 10.0,
            edge_radius_um: if s.is_high_end { wp.edge_radius_um } else { wp.edge_radius_um * 0.7 },
            core_ratio: core,
            runout_um: s.runout_um.unwrap_or(if s.is_high_end { 3.0 } else { 10.0 }),
            variable_pitch: s.variable_pitch,
            elastic_gpa: e,
            tool_conductivity: k,
            tool_expansion: alpha,
            tool_rho_c: rho_c,
            base_wear_factor: wf,
            coating: tribology_from_name(s.coating_name.as_deref()),
            holder_stiffness_n_per_um: machine.holder_stiffness_n_per_um,
        }
    }

    pub fn radius(&self) -> f64 {
        self.diameter_mm / 2.0
    }

    pub fn local_radius(&self, z_from_tip: f64) -> (f64, f64) {
        let r = self.radius();
        let rc = self.nose.corner_radius(self.diameter_mm);
        if rc <= 0.0 || z_from_tip >= rc {
            return (r, 1.0);
        }
        let dz = rc - z_from_tip.max(0.0);
        let local = (r - rc) + (rc * rc - dz * dz).max(0.0).sqrt();
        let sin_k = ((rc * rc - dz * dz).max(0.0).sqrt() / rc).clamp(0.05, 1.0);
        (local.max(1e-4), sin_k)
    }

    pub fn tool_diffusivity(&self) -> f64 {
        self.tool_conductivity / self.tool_rho_c
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoolantState {
    pub method: CoolantMethod,
    pub pressure_bar: f64,
    pub flow_l_min: f64,
    pub temperature_c: f64,
}

impl CoolantState {
    pub fn from_config(c: &CoolantConfig) -> Self {
        Self {
            method: c.method.clone(),
            pressure_bar: c.pressure_bar,
            flow_l_min: c.flow_rate_l_min,
            temperature_c: c.temperature_c.unwrap_or(22.0),
        }
    }

    pub fn from_type(t: &CoolantType) -> Self {
        let cfg = match t {
            CoolantType::Flood => CoolantConfig::flood(),
            CoolantType::Mist => CoolantConfig::mist(),
            CoolantType::AirBlast => CoolantConfig::air_blast(),
            CoolantType::Dry => CoolantConfig::dry(),
            CoolantType::ThroughTool => CoolantConfig::through_tool(),
        };
        Self::from_config(&cfg)
    }

    pub fn lubricity(&self) -> f64 {
        match self.method {
            CoolantMethod::Dry | CoolantMethod::AirBlast => 1.0,
            CoolantMethod::Mist => 0.85,
            CoolantMethod::Flood => 0.92,
            CoolantMethod::ThroughTool => 0.90,
        }
    }

    pub fn interface_effectiveness(&self, vc: f64) -> f64 {
        match self.method {
            CoolantMethod::Dry => 0.0,
            CoolantMethod::AirBlast => 0.04 * (self.pressure_bar / 6.0).clamp(0.3, 1.5),
            CoolantMethod::Mist => 0.06,
            CoolantMethod::Flood => {
                let flow = (self.flow_l_min / 12.0).clamp(0.3, 1.5).powf(0.3);
                0.25 * flow / (1.0 + vc / 150.0)
            }
            CoolantMethod::ThroughTool => {
                let p = (self.pressure_bar / 20.0).max(0.05).powf(0.3).min(1.3);
                (0.35 * p / (1.0 + vc / 300.0)).min(0.4)
            }
        }
    }

    pub fn body_effectiveness(&self) -> f64 {
        match self.method {
            CoolantMethod::Dry => 0.0,
            CoolantMethod::AirBlast => 0.3,
            CoolantMethod::Mist => 0.4,
            CoolantMethod::Flood => 0.8,
            CoolantMethod::ThroughTool => 0.85,
        }
    }

    pub fn quench_strength(&self) -> f64 {
        match self.method {
            CoolantMethod::Dry => 0.05,
            CoolantMethod::AirBlast => 0.2,
            CoolantMethod::Mist => 0.3,
            CoolantMethod::Flood => 1.0,
            CoolantMethod::ThroughTool => 1.0,
        }
    }

    pub fn workpiece_h(&self) -> f64 {
        20.0 + match self.method {
            CoolantMethod::Dry => 10.0,
            CoolantMethod::AirBlast => 40.0,
            CoolantMethod::Mist => 80.0,
            CoolantMethod::Flood => 1000.0 * (self.flow_l_min / 12.0).clamp(0.3, 1.5),
            CoolantMethod::ThroughTool => 300.0,
        }
    }

    pub fn chip_evacuation(&self) -> f64 {
        match self.method {
            CoolantMethod::Dry => 0.3,
            CoolantMethod::AirBlast => 0.7,
            CoolantMethod::Mist => 0.5,
            CoolantMethod::Flood => 0.8,
            CoolantMethod::ThroughTool => 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Calibration {
    pub power: f64,
    pub wear: f64,
    pub deflection: f64,
    pub temperature: f64,
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            power: 1.0,
            wear: 1.0,
            deflection: 1.0,
            temperature: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CutContext {
    pub wp: WorkpieceProps,
    pub tool: ToolGeometry,
    pub coolant: CoolantState,
    pub machine: MachineLimits,
    pub calib: Calibration,
    pub tolerance_mm: f64,
    pub allowance_mm: f64,
}

impl CutContext {
    pub fn from_profile(p: &MachiningProfile) -> Self {
        let wp = WorkpieceProps::of(&p.workpiece_setup.effective_material());
        let tool = ToolGeometry::from_setting(&p.endmill_setting, &wp, &p.machine, None);
        Self {
            coolant: CoolantState::from_config(&p.coolant_config),
            machine: p.machine.clone(),
            calib: Calibration::default(),
            tolerance_mm: p.workpiece_setup.effective_tolerance_mm(),
            allowance_mm: p.workpiece_setup.stock_allowance_mm.max(0.0),
            wp,
            tool,
        }
    }

    pub fn vb_limit_mm(&self) -> f64 {
        if self.tolerance_mm <= 0.02 {
            0.2
        } else {
            0.3
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum MillMode {
    Up,
    Down,
    Slot,
    Center,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Engagement {
    pub ap_mm: f64,
    pub ae_mm: f64,
    pub phi_st: f64,
    pub phi_ex: f64,
    pub mode: MillMode,
}

impl Engagement {
    pub fn side(ap: f64, ae: f64, d: f64, down: bool) -> Self {
        let ae = ae.clamp(0.0, d);
        if ae >= 0.999 * d {
            return Self {
                ap_mm: ap,
                ae_mm: d,
                phi_st: 0.0,
                phi_ex: PI,
                mode: MillMode::Slot,
            };
        }
        let phis = (1.0 - 2.0 * ae / d).clamp(-1.0, 1.0).acos();
        if down {
            Self {
                ap_mm: ap,
                ae_mm: ae,
                phi_st: PI - phis,
                phi_ex: PI,
                mode: MillMode::Down,
            }
        } else {
            Self {
                ap_mm: ap,
                ae_mm: ae,
                phi_st: 0.0,
                phi_ex: phis,
                mode: MillMode::Up,
            }
        }
    }

    pub fn centered(ap: f64, ae: f64, d: f64) -> Self {
        let phis = (1.0 - 2.0 * ae.clamp(0.0, d) / d).clamp(-1.0, 1.0).acos();
        Self {
            ap_mm: ap,
            ae_mm: ae,
            phi_st: PI / 2.0 - phis / 2.0,
            phi_ex: PI / 2.0 + phis / 2.0,
            mode: MillMode::Center,
        }
    }

    pub fn span(&self) -> f64 {
        (self.phi_ex - self.phi_st).max(0.0)
    }

    pub fn max_sin(&self) -> f64 {
        if self.phi_st <= PI / 2.0 && self.phi_ex >= PI / 2.0 {
            1.0
        } else {
            self.phi_st.sin().max(self.phi_ex.sin())
        }
    }

    pub fn wall_angle(&self) -> Option<f64> {
        match self.mode {
            MillMode::Up => Some(0.0),
            MillMode::Down => Some(PI),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuttingCoeffs {
    pub mu: f64,
    pub beta_deg: f64,
    pub shear_angle_deg: f64,
    pub kc11: f64,
    pub mc: f64,
    pub kr: f64,
    pub kte: f64,
    pub kre: f64,
    pub friction_factor: f64,
    pub friction_power_share: f64,
}

pub fn cutting_coeffs(ctx: &CutContext) -> CuttingCoeffs {
    let mu = (ctx.tool.coating.friction * ctx.wp.adhesion * ctx.coolant.lubricity()).clamp(0.05, 1.2);
    let beta = mu.atan();
    let alpha = ctx.tool.rake_deg.to_radians();
    let x = (beta - alpha).clamp((-5f64).to_radians(), 50f64.to_radians());
    let g = |v: f64| 2.0 * v.cos() / (1.0 - v.sin());
    let x0 = 0.55f64.atan() - 6f64.to_radians();
    let factor = (g(x) / g(x0)).clamp(0.6, 1.6);
    let phi = PI / 4.0 - x / 2.0;
    let share = (beta.sin() * phi.sin() / ((beta - alpha).cos() * (phi - alpha).cos())).clamp(0.05, 0.8);
    let kc11 = ctx.wp.kc11 * factor * ctx.calib.power;
    let kte = 0.011 * ctx.wp.kc11 * (ctx.tool.edge_radius_um / 10.0).max(0.1).sqrt() * ctx.calib.power;
    CuttingCoeffs {
        mu,
        beta_deg: beta.to_degrees(),
        shear_angle_deg: phi.to_degrees(),
        kc11,
        mc: ctx.wp.mc,
        kr: x.tan().clamp(0.15, 0.9),
        kte,
        kre: 1.4 * kte,
        friction_factor: factor,
        friction_power_share: share,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForceResult {
    pub fx_mean: f64,
    pub fy_mean: f64,
    pub f_res_mean: f64,
    pub f_res_peak: f64,
    pub f_normal_peak: f64,
    pub torque_mean_nm: f64,
    pub torque_peak_nm: f64,
    pub cutting_power_kw: f64,
    pub h_max_mm: f64,
    pub h_mean_mm: f64,
    pub teeth_in_cut: f64,
    pub radial_load_n_per_mm: f64,
    pub fx_series: Vec<f64>,
    pub fy_series: Vec<f64>,
    pub pitch_rad: f64,
}

pub fn mechanistic_forces(tool: &ToolGeometry, co: &CuttingCoeffs, fz: f64, rpm: f64, eng: &Engagement, steps: usize, slices: usize) -> ForceResult {
    let mut out = ForceResult::default();
    if eng.ap_mm <= 0.0 || eng.span() <= 0.0 || fz <= 0.0 {
        return out;
    }
    let z = tool.flutes.max(1) as usize;
    let pitch = 2.0 * PI / z as f64;
    let r = tool.radius();
    let tan_h = tool.helix_deg.to_radians().tan();
    let dz = eng.ap_mm / slices as f64;
    let runout_mm = tool.runout_um / 1000.0;
    let mut fx_s = Vec::with_capacity(steps);
    let mut fy_s = Vec::with_capacity(steps);
    let mut t_sum = 0.0;
    let mut t_peak = 0.0f64;
    let mut fres_peak = 0.0f64;
    let mut fn_peak = 0.0f64;
    let mut h_sum = 0.0;
    let mut h_cnt = 0.0;
    let mut h_max = 0.0f64;
    let mut teeth_sum = 0.0;
    let mut fr_sum = 0.0;
    let mut len_sum = 0.0;
    for s in 0..steps {
        let theta = pitch * s as f64 / steps as f64;
        let mut fx = 0.0;
        let mut fy = 0.0;
        let mut torque = 0.0;
        let mut teeth = vec![false; z];
        for j in 0..z {
            let offset = if z > 1 {
                let cur = (2.0 * PI * j as f64 / z as f64).cos();
                let prev = (2.0 * PI * ((j + z - 1) % z) as f64 / z as f64).cos();
                runout_mm * (cur - prev)
            } else {
                0.0
            };
            for k in 0..slices {
                let zk = (k as f64 + 0.5) * dz;
                let (rl, sin_k) = tool.local_radius(zk);
                let lag = zk * tan_h / r;
                let mut phi = theta + j as f64 * pitch - lag;
                phi = phi.rem_euclid(2.0 * PI);
                if phi < eng.phi_st || phi > eng.phi_ex {
                    continue;
                }
                let h = ((fz * phi.sin() + offset) * sin_k).max(0.0);
                if h <= 1e-7 {
                    continue;
                }
                teeth[j] = true;
                let db = dz / sin_k;
                let shear = co.kc11 * h.powf(1.0 - co.mc) * db;
                let dft = shear + co.kte * db;
                let dfr = co.kr * shear + co.kre * db;
                let (sp, cp) = phi.sin_cos();
                fx += -dft * cp - dfr * sp;
                fy += dft * sp - dfr * cp;
                torque += dft * rl;
                h_sum += h;
                h_cnt += 1.0;
                h_max = h_max.max(h);
                fr_sum += dfr;
                len_sum += db;
            }
        }
        teeth_sum += teeth.iter().filter(|t| **t).count() as f64;
        let fres = (fx * fx + fy * fy).sqrt();
        fres_peak = fres_peak.max(fres);
        fn_peak = fn_peak.max(fy.abs());
        t_peak = t_peak.max(torque);
        t_sum += torque;
        fx_s.push(fx);
        fy_s.push(fy);
    }
    let n = steps as f64;
    out.fx_mean = fx_s.iter().sum::<f64>() / n;
    out.fy_mean = fy_s.iter().sum::<f64>() / n;
    out.f_res_mean = fx_s.iter().zip(fy_s.iter()).map(|(a, b)| (a * a + b * b).sqrt()).sum::<f64>() / n;
    out.f_res_peak = fres_peak;
    out.f_normal_peak = fn_peak;
    out.torque_mean_nm = t_sum / n / 1000.0;
    out.torque_peak_nm = t_peak / 1000.0;
    out.cutting_power_kw = out.torque_mean_nm * 2.0 * PI * rpm / 60.0 / 1000.0;
    out.h_max_mm = h_max;
    out.h_mean_mm = if h_cnt > 0.0 { h_sum / h_cnt } else { 0.0 };
    out.teeth_in_cut = teeth_sum / n;
    out.radial_load_n_per_mm = if len_sum > 0.0 { fr_sum / len_sum } else { 0.0 };
    out.fx_series = fx_s;
    out.fy_series = fy_s;
    out.pitch_rad = pitch;
    out
}

pub fn plunge_forces(tool: &ToolGeometry, co: &CuttingCoeffs, fz_axial: f64, rpm: f64) -> ForceResult {
    let mut out = ForceResult::default();
    if fz_axial <= 0.0 {
        return out;
    }
    let r = tool.radius();
    let z = tool.flutes.max(1) as f64;
    let ft_edge = co.kc11 * fz_axial.powf(1.0 - co.mc) * r + co.kte * r;
    let torque = z * ft_edge * r / 2.0 / 1000.0;
    out.torque_mean_nm = torque;
    out.torque_peak_nm = torque * 1.15;
    out.cutting_power_kw = torque * 2.0 * PI * rpm / 60.0 / 1000.0;
    out.f_res_mean = z * co.kr * ft_edge * 0.15;
    out.f_res_peak = out.f_res_mean * 1.5;
    out.h_max_mm = fz_axial;
    out.h_mean_mm = fz_axial;
    out.teeth_in_cut = z;
    out.radial_load_n_per_mm = co.kr * ft_edge / r.max(1e-6);
    out.pitch_rad = 2.0 * PI / z;
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compliance {
    pub stiffness_n_per_um: f64,
    pub load_point_from_holder_mm: f64,
    pub ei_equivalent: f64,
}

pub fn tool_compliance(tool: &ToolGeometry, ap: f64, cal: f64) -> Compliance {
    let e = tool.elastic_gpa * 1000.0;
    let d_f = tool.core_ratio * tool.diameter_mm;
    let i_f = PI * d_f.powi(4) / 64.0;
    let i_s = PI * tool.shank_mm.max(d_f).powi(4) / 64.0;
    let l = tool.stickout_mm.max(tool.loc_mm.min(tool.stickout_mm));
    let lf = tool.loc_mm.min(l);
    let ls = (l - lf).max(0.0);
    let a = (l - ap.max(0.0).min(l) / 2.0).max(1e-3);
    let delta_per_n = if a <= ls {
        a.powi(3) / (3.0 * e * i_s)
    } else {
        let tail = a - ls;
        (a.powi(3) - tail.powi(3)) / (3.0 * e * i_s) + tail.powi(3) / (3.0 * e * i_f)
    };
    let holder = 1.0 / (tool.holder_stiffness_n_per_um * 1000.0);
    let total = (delta_per_n + holder) * cal;
    Compliance {
        stiffness_n_per_um: 1.0 / (total * 1000.0),
        load_point_from_holder_mm: a,
        ei_equivalent: a.powi(3) / (3.0 * delta_per_n.max(1e-18)),
    }
}

pub fn deflection_at(comp: &Compliance, force_n: f64, x_from_holder: f64, holder_part: f64) -> f64 {
    let a = comp.load_point_from_holder_mm;
    let ei = comp.ei_equivalent;
    let beam = if x_from_holder >= a {
        force_n * a * a * (3.0 * x_from_holder - a) / (6.0 * ei)
    } else {
        force_n * x_from_holder * x_from_holder * (3.0 * a - x_from_holder) / (6.0 * ei)
    };
    (beam + force_n * holder_part) * 1000.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalResult {
    pub interface_dry_c: f64,
    pub interface_c: f64,
    pub coolant_reduction_pct: f64,
    pub tool_body_c: f64,
    pub tool_radial_growth_um: f64,
    pub tool_axial_growth_um: f64,
    pub engagement_time_ms: f64,
    pub transient_factor: f64,
    pub workpiece_heat_w: f64,
    pub workpiece_steady_rise_c: f64,
    pub workpiece_time_constant_s: f64,
    pub thermal_crack_risk: f64,
    pub bue_risk: f64,
    pub over_coating_limit: bool,
    pub over_workpiece_limit: bool,
}

pub fn thermal(ctx: &CutContext, co: &CuttingCoeffs, f: &ForceResult, vc: f64, rpm: f64, eng: &Engagement, wp_mass_kg: f64, wp_area_m2: f64) -> ThermalResult {
    let h = f.h_mean_mm.max(1e-4);
    let u = co.kc11 * h.powf(-co.mc) * 1.0e6;
    let v = vc / 60.0;
    let pe = (v * h * 1e-3 / ctx.wp.diffusivity()).max(1e-6);
    let mut dt = 0.4 * u / ctx.wp.rho_c() * pe.cbrt() * ctx.wp.thermal_factor * ctx.calib.temperature;
    let t0 = if matches!(ctx.coolant.method, CoolantMethod::Dry | CoolantMethod::AirBlast) {
        22.0
    } else {
        ctx.coolant.temperature_c
    };
    let cap = 0.9 * ctx.wp.melt_c - t0;
    dt = dt.min(cap.max(50.0));
    let t_cut = if rpm > 0.0 { eng.span() / (2.0 * PI) * 60.0 / rpm } else { 0.0 };
    let lc = (3.0 * f.h_max_mm).max(0.05) * 1e-3;
    let tau = lc * lc / ctx.tool.tool_diffusivity();
    let tf = if t_cut > 0.0 { 1.0 - (-t_cut / tau).exp() } else { 1.0 };
    let dry = t0 + dt * tf;
    let eta = ctx.coolant.interface_effectiveness(vc);
    let wet = t0 + dt * tf * (1.0 - eta);
    let body = t0 + 0.08 * (wet - t0) * (1.0 - ctx.coolant.body_effectiveness());
    let r_wp = 0.1 + 0.2 / (1.0 + vc / 100.0);
    let p_wp = f.cutting_power_kw * 1000.0 * r_wp;
    let ha = ctx.coolant.workpiece_h() * wp_area_m2.max(1e-4);
    let mc = wp_mass_kg.max(1e-3) * ctx.wp.specific_heat;
    let quench = ctx.coolant.quench_strength();
    let interrupted = eng.span() < 1.9 * PI;
    let crack = if interrupted && ctx.tool.base_wear_factor <= 1.0 {
        (((wet - ctx.coolant.temperature_c) * quench - 350.0) / 400.0).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let bue = match ctx.wp.bue_range_c {
        Some((lo, hi)) if wet >= lo && wet <= hi => (0.8 * (1.0 - 2.0 * (1.0 - ctx.coolant.lubricity()))).clamp(0.1, 0.8),
        _ => 0.0,
    };
    ThermalResult {
        interface_dry_c: dry,
        interface_c: wet,
        coolant_reduction_pct: if dry - t0 > 0.0 { (dry - wet) / (dry - t0) * 100.0 } else { 0.0 },
        tool_body_c: body,
        tool_radial_growth_um: ctx.tool.tool_expansion * ctx.tool.radius() * (body - 22.0) * 1000.0,
        tool_axial_growth_um: ctx.tool.tool_expansion * ctx.tool.stickout_mm * (body - 22.0) * 0.5 * 1000.0,
        engagement_time_ms: t_cut * 1000.0,
        transient_factor: tf,
        workpiece_heat_w: p_wp,
        workpiece_steady_rise_c: p_wp / ha,
        workpiece_time_constant_s: mc / ha,
        thermal_crack_risk: crack,
        bue_risk: bue,
        over_coating_limit: wet > 0.85 * ctx.tool.coating.max_temp_c,
        over_workpiece_limit: ctx.wp.temp_limit_c.map(|l| wet > l).unwrap_or(false),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WearResult {
    pub vb_rate_mm_per_min: f64,
    pub tool_life_min: f64,
    pub radial_loss_rate_um_per_min: f64,
    pub vb_limit_mm: f64,
    pub temperature_factor: f64,
    pub stress_factor: f64,
    pub speed_factor: f64,
    pub coating_factor: f64,
    pub runout_factor: f64,
}

pub fn reference_state(ctx: &CutContext) -> (f64, f64, f64, f64) {
    let mut rctx = ctx.clone();
    rctx.coolant = CoolantState::from_config(&rctx.wp.reference_coolant());
    rctx.calib = Calibration::default();
    rctx.tool.coating = tribology_from_name(Some("AlTiN"));
    rctx.tool.runout_um = 3.0;
    rctx.tool.base_wear_factor = 1.0;
    rctx.tool.diameter_mm = 10.0;
    rctx.tool.loc_mm = 25.0;
    rctx.tool.stickout_mm = 35.0;
    rctx.tool.flutes = 4;
    rctx.tool.nose = ToolNose::Square;
    let co = cutting_coeffs(&rctx);
    let d = 10.0;
    let fz = (rctx.wp.fz_coeff * d).clamp(0.003, 0.25);
    let ae = rctx.wp.ae_ratio * d;
    let eng = Engagement::side(rctx.wp.ap_ratio * d, ae, d, true);
    let rpm = rctx.wp.ref_vc * 1000.0 / (PI * d);
    let f = mechanistic_forces(&rctx.tool, &co, fz, rpm, &eng, 48, 8);
    let th = thermal(&rctx, &co, &f, rctx.wp.ref_vc, rpm, &eng, 1.0, 0.03);
    (th.interface_c + 273.15, f.radial_load_n_per_mm.max(1e-6), rctx.wp.ref_vc, eng.span() / (2.0 * PI))
}

pub fn wear(ctx: &CutContext, f: &ForceResult, th: &ThermalResult, vc: f64, fz: f64, eng: &Engagement, reference: (f64, f64, f64, f64)) -> WearResult {
    let (t_ref, q_ref, v_ref, duty_ref) = reference;
    let t = th.interface_c + 273.15;
    let tf = (-ctx.wp.wear_activation_k * (1.0 / t - 1.0 / t_ref)).exp().clamp(0.01, 100.0);
    let sf = (f.radial_load_n_per_mm / q_ref).clamp(0.05, 20.0);
    let duty = eng.span() / (2.0 * PI);
    let vf = (vc / v_ref) * (duty / duty_ref.max(1e-6));
    let cf = ctx.tool.coating.wear_factor / 0.4
        * if !ctx.tool.coating.ferrous_ok && ctx.wp.key != "aluminum" && ctx.wp.key != "cfrp" {
            4.0
        } else {
            1.0
        };
    let rf = (1.0 + 2.0 * ctx.tool.runout_um / 1000.0 / fz.max(1e-4)).powf(1.0 - ctx.wp.mc).min(2.0);
    let evac = 1.0 + 0.3 * (1.0 - ctx.coolant.chip_evacuation()) * if ctx.wp.ae_ratio >= 0.5 || eng.mode == MillMode::Slot { 1.0 } else { 0.3 };
    let base_rate = 0.2 / ctx.wp.ref_life_min.max(1.0);
    let mix = 0.25 + 0.75 * tf;
    let rate = base_rate * mix * sf * vf * cf * rf * ctx.tool.base_wear_factor * evac * ctx.calib.wear
        * (1.0 + th.bue_risk * 0.5)
        * (1.0 + 1.5 * th.thermal_crack_risk);
    let limit = ctx.vb_limit_mm();
    WearResult {
        vb_rate_mm_per_min: rate,
        tool_life_min: if rate > 0.0 { limit / rate } else { f64::INFINITY },
        radial_loss_rate_um_per_min: rate * ctx.tool.clearance_deg.to_radians().tan() * 1000.0,
        vb_limit_mm: limit,
        temperature_factor: mix,
        stress_factor: sf,
        speed_factor: vf,
        coating_factor: cf,
        runout_factor: rf,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurfaceResult {
    pub wall_ra_um: f64,
    pub feed_mark_um: f64,
    pub scallop_um: f64,
}

pub fn surface(tool: &ToolGeometry, fz: f64, stepover: f64) -> SurfaceResult {
    let r = tool.radius();
    let rt = fz * fz / (8.0 * r) * 1000.0;
    let ra = rt / 4.0 + tool.runout_um / 4.0;
    let rc = tool.nose.corner_radius(tool.diameter_mm);
    let scallop = if rc > 0.0 && stepover > 0.0 && stepover < 2.0 * rc {
        (rc - (rc * rc - stepover * stepover / 4.0).sqrt()) * 1000.0
    } else {
        0.0
    };
    SurfaceResult {
        wall_ra_um: ra,
        feed_mark_um: rt,
        scallop_um: scallop,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WallError {
    pub deflection_um: f64,
    pub deflection_top_um: f64,
    pub deflection_bottom_um: f64,
    pub wear_um_per_min: f64,
    pub thermal_tool_um: f64,
}

pub fn wall_error(ctx: &CutContext, f: &ForceResult, eng: &Engagement, th: &ThermalResult, wr: &WearResult) -> WallError {
    let comp = tool_compliance(&ctx.tool, eng.ap_mm, ctx.calib.deflection);
    let holder = 1.0 / (ctx.tool.holder_stiffness_n_per_um * 1000.0) * ctx.calib.deflection;
    let l = ctx.tool.stickout_mm;
    let mut top = 0.0;
    let mut bottom = 0.0;
    let mut mean = 0.0;
    let rc = ctx.tool.nose.corner_radius(ctx.tool.diameter_mm);
    if rc > 0.0 && eng.ap_mm < rc && ctx.tool.nose == ToolNose::Ball {
        let d = deflection_at(&comp, f.f_res_mean, (l - eng.ap_mm / 2.0).max(0.0), holder);
        return WallError {
            deflection_um: d,
            deflection_top_um: d,
            deflection_bottom_um: d,
            wear_um_per_min: wr.radial_loss_rate_um_per_min,
            thermal_tool_um: -th.tool_radial_growth_um,
        };
    }
    if let Some(wall) = eng.wall_angle() {
        let steps = f.fy_series.len().max(1);
        let slices = 8;
        let r = ctx.tool.radius();
        let tan_h = ctx.tool.helix_deg.to_radians().tan();
        for k in 0..slices {
            let zk = (k as f64 + 0.5) * eng.ap_mm / slices as f64;
            let theta = (wall + zk * tan_h / r).rem_euclid(f.pitch_rad.max(1e-9));
            let idx = ((theta / f.pitch_rad) * steps as f64).floor() as usize % steps;
            let fy = f.fy_series.get(idx).cloned().unwrap_or(0.0);
            let away = if eng.mode == MillMode::Down { fy } else { -fy };
            let d = deflection_at(&comp, away, (l - zk).max(0.0), holder);
            mean += d / slices as f64;
            if k == 0 {
                bottom = d;
            }
            if k == slices - 1 {
                top = d;
            }
        }
    }
    WallError {
        deflection_um: mean,
        deflection_top_um: top,
        deflection_bottom_um: bottom,
        wear_um_per_min: wr.radial_loss_rate_um_per_min,
        thermal_tool_um: -th.tool_radial_growth_um,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CutAnalysis {
    pub rpm: f64,
    pub vc_m_min: f64,
    pub fz_mm: f64,
    pub feed_mm_min: f64,
    pub ap_mm: f64,
    pub ae_mm: f64,
    pub mode: MillMode,
    pub engage_angle_deg: f64,
    pub chip_thinning_factor: f64,
    pub mrr_cm3_min: f64,
    pub vc_effective_m_min: f64,
    pub coeffs: CuttingCoeffs,
    pub forces: ForceSummary,
    pub spindle_power_kw: f64,
    pub available_power_kw: f64,
    pub stiffness_n_per_um: f64,
    pub deflection_peak_um: f64,
    pub wall: WallError,
    pub thermal: ThermalResult,
    pub wear: WearResult,
    pub surface: SurfaceResult,
    pub tolerance_mm: f64,
    pub error_budget_um: ErrorBudget,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForceSummary {
    pub fx_mean: f64,
    pub fy_mean: f64,
    pub f_res_mean: f64,
    pub f_res_peak: f64,
    pub torque_mean_nm: f64,
    pub torque_peak_nm: f64,
    pub cutting_power_kw: f64,
    pub h_max_mm: f64,
    pub h_mean_mm: f64,
    pub teeth_in_cut: f64,
}

impl From<&ForceResult> for ForceSummary {
    fn from(f: &ForceResult) -> Self {
        Self {
            fx_mean: f.fx_mean,
            fy_mean: f.fy_mean,
            f_res_mean: f.f_res_mean,
            f_res_peak: f.f_res_peak,
            torque_mean_nm: f.torque_mean_nm,
            torque_peak_nm: f.torque_peak_nm,
            cutting_power_kw: f.cutting_power_kw,
            h_max_mm: f.h_max_mm,
            h_mean_mm: f.h_mean_mm,
            teeth_in_cut: f.teeth_in_cut,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBudget {
    pub deflection_um: f64,
    pub wear_10min_um: f64,
    pub thermal_tool_um: f64,
    pub thermal_workpiece_um: f64,
    pub total_um: f64,
    pub tolerance_um: f64,
    pub utilization: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ThermalBody {
    pub mass_kg: f64,
    pub area_m2: f64,
    pub size_mm: f64,
}

impl ThermalBody {
    pub fn of_setup(w: &crate::workpiece_setup::WorkpieceSetup, wp: &WorkpieceProps) -> Self {
        Self {
            mass_kg: (w.volume_cm3().max(1.0) * 1e-6) * wp.density,
            area_m2: w.surface_area_m2().max(1e-4),
            size_mm: w.width_mm.max(w.height_mm),
        }
    }
}

pub fn effective_vc(tool: &ToolGeometry, rpm: f64, ap: f64) -> f64 {
    let (rl, _) = tool.local_radius(ap.max(0.0));
    PI * 2.0 * rl * rpm / 1000.0
}

pub fn analyze_cut(ctx: &CutContext, conds: &CuttingConditions, down: bool, body: ThermalBody) -> CutAnalysis {
    let d = ctx.tool.diameter_mm;
    let rpm = conds.spindle_rpm as f64;
    let vc = PI * d * rpm / 1000.0;
    let z = ctx.tool.flutes.max(1) as f64;
    let fz = if rpm > 0.0 { conds.feed_rate_mm_min / (rpm * z) } else { conds.feed_per_tooth_mm };
    let ap = conds.axial_doc_mm.min(ctx.tool.loc_mm);
    let eng = Engagement::side(ap, conds.radial_doc_mm, d, down);
    let co = cutting_coeffs(ctx);
    let f = mechanistic_forces(&ctx.tool, &co, fz, rpm, &eng, 72, 12);
    let comp = tool_compliance(&ctx.tool, ap, ctx.calib.deflection);
    let vce = effective_vc(&ctx.tool, rpm, ap);
    let th = thermal(ctx, &co, &f, vce, rpm, &eng, body.mass_kg, body.area_m2);
    let reference = reference_state(ctx);
    let wr = wear(ctx, &f, &th, vce, fz, &eng, reference);
    let sf = surface(&ctx.tool, fz, conds.radial_doc_mm);
    let wall = wall_error(ctx, &f, &eng, &th, &wr);
    let spindle = f.cutting_power_kw / ctx.machine.efficiency.max(0.1);
    let avail = ctx.machine.available_power_kw(rpm);
    let defl_peak = f.f_res_peak / comp.stiffness_n_per_um;
    let wear10 = wr.radial_loss_rate_um_per_min * 10.0;
    let wp_rise = th.workpiece_steady_rise_c * (1.0 - (-600.0 / th.workpiece_time_constant_s.max(1.0)).exp());
    let wp_um = ctx.wp.expansion * body.size_mm * wp_rise * 1000.0;
    let wall_defl = wall.deflection_um;
    let total = wall_defl.abs() + wear10 + th.tool_radial_growth_um.abs() + wp_um;
    let tol_um = ctx.tolerance_mm * 1000.0;
    let mut notes = Vec::new();
    let thinning = if eng.mode != MillMode::Slot && eng.max_sin() > 0.0 { 1.0 / eng.max_sin() } else { 1.0 };
    if thinning > 1.3 {
        notes.push(format!(
            "반경 절입이 작아 실제 최대 칩두께가 프로그램 fz 의 {:.0}% 입니다 (칩 얇아짐).",
            100.0 / thinning
        ));
    }
    if f.h_max_mm < ctx.tool.edge_radius_um / 1000.0 {
        notes.push("최대 칩두께가 날끝 반경보다 작아 밀림(ploughing)·가공경화 위험이 있습니다.".into());
    }
    if th.over_coating_limit {
        notes.push(format!(
            "날끝 온도 {:.0}°C 가 코팅 허용온도({:.0}°C)의 85% 를 넘습니다.",
            th.interface_c, ctx.tool.coating.max_temp_c
        ));
    }
    if th.thermal_crack_risk > 0.45 {
        notes.push("단속 절삭 + 급랭 조합으로 열균열(comb crack) 위험이 있습니다. 건식/에어 전환을 검토하세요.".into());
    }
    if th.bue_risk > 0.3 {
        notes.push("구성인선(BUE) 형성 온도 구간입니다. 절삭속도 상향 또는 윤활을 강화하세요.".into());
    }
    if spindle > avail {
        notes.push(format!("필요 동력 {:.2} kW 가 해당 회전수의 가용 동력 {:.2} kW 를 초과합니다.", spindle, avail));
    }
    CutAnalysis {
        rpm,
        vc_m_min: vc,
        fz_mm: fz,
        feed_mm_min: conds.feed_rate_mm_min,
        ap_mm: ap,
        ae_mm: eng.ae_mm,
        mode: eng.mode,
        engage_angle_deg: eng.span().to_degrees(),
        chip_thinning_factor: thinning,
        mrr_cm3_min: ap * eng.ae_mm * conds.feed_rate_mm_min / 1000.0,
        vc_effective_m_min: vce,
        coeffs: co,
        forces: ForceSummary::from(&f),
        spindle_power_kw: spindle,
        available_power_kw: avail,
        stiffness_n_per_um: comp.stiffness_n_per_um,
        deflection_peak_um: defl_peak,
        wall,
        surface: sf,
        tolerance_mm: ctx.tolerance_mm,
        error_budget_um: ErrorBudget {
            deflection_um: wall_defl,
            wear_10min_um: wear10,
            thermal_tool_um: th.tool_radial_growth_um,
            thermal_workpiece_um: wp_um,
            total_um: total,
            tolerance_um: tol_um,
            utilization: if tol_um > 0.0 { total / tol_um } else { 0.0 },
        },
        thermal: th,
        wear: wr,
        notes,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Purpose {
    Roughing,
    Finishing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub purpose: Purpose,
    pub conditions: CuttingConditions,
    pub fz_target_mm: f64,
    pub chip_thinning_factor: f64,
    pub deflection_budget_um: f64,
    pub predicted_wall_error_um: f64,
    pub predicted_tool_life_min: f64,
    pub notes: Vec<String>,
}

pub fn engagement_defaults(ctx: &CutContext, purpose: Purpose) -> (f64, f64) {
    let d = ctx.tool.diameter_mm;
    let hard = ctx.wp.hardness_hrc >= 45.0;
    match (ctx.tool.nose, purpose) {
        (ToolNose::Ball, Purpose::Roughing) => (if hard { 0.06 * d } else { 0.1 * d }, 0.1 * d),
        (ToolNose::Ball, Purpose::Finishing) => (if hard { 0.04 * d } else { 0.06 * d }, 0.05 * d),
        (_, Purpose::Roughing) => ((ctx.wp.ap_ratio * d).min(0.9 * ctx.tool.loc_mm), ctx.wp.ae_ratio * d),
        (_, Purpose::Finishing) => (
            (ctx.wp.ap_ratio * d).min(0.9 * ctx.tool.loc_mm),
            if ctx.allowance_mm > 0.0 { ctx.allowance_mm.min(0.2 * d) } else { 0.05 * d },
        ),
    }
}

pub fn recommend(ctx: &CutContext, is_high_end: bool, body: ThermalBody, purpose: Purpose) -> Recommendation {
    let d = ctx.tool.diameter_mm;
    let z = ctx.tool.flutes.max(1) as f64;
    let mut notes = Vec::new();
    let tier = if is_high_end { 1.0 } else { 0.55 };
    let coat = match ctx.tool.coating.family.as_str() {
        "uncoated" => 0.6,
        "TiN" | "CrN" | "ZrN" => 0.8,
        "TiCN" => 0.85,
        "nACo" | "TiSiN" | "AlCrN" => 1.1,
        _ => 1.0,
    };
    let dry_penalty = if matches!(ctx.coolant.method, CoolantMethod::Dry) && ctx.wp.thermal_factor < 0.9 && ctx.wp.hardness_hrc < 45.0 {
        0.75
    } else {
        1.0
    };
    let (mut ap, mut ae) = engagement_defaults(ctx, purpose);
    let vc_target = ctx.wp.ref_vc * tier * coat * dry_penalty;
    let d_eff = 2.0 * ctx.tool.local_radius(ap).0;
    let mut rpm = vc_target * 1000.0 / (PI * d_eff.max(0.1 * d));
    if rpm > ctx.machine.max_rpm {
        rpm = ctx.machine.max_rpm;
        notes.push(format!(
            "최대 회전수 {:.0} rpm 으로 제한되어 유효 절삭속도가 {:.0} m/min 입니다.",
            ctx.machine.max_rpm,
            PI * d_eff * rpm / 1000.0
        ));
    }
    let fz_target = (ctx.wp.fz_coeff * d).clamp(0.003, 0.25)
        * if is_high_end { 1.0 } else { 0.8 }
        * if purpose == Purpose::Finishing { 0.8 } else { 1.0 };
    let budget_um = match purpose {
        Purpose::Roughing => (0.5 * ctx.allowance_mm * 1000.0).max(20.0),
        Purpose::Finishing => (0.3 * ctx.tolerance_mm * 1000.0).max(2.0),
    };
    let mut fz_scale: f64 = 1.0;
    let mut thinning;
    let mut fz;
    let mut conds;
    let mut last;
    let mut iter = 0;
    loop {
        let sin_k = ctx.tool.local_radius(ap).1;
        let ae_eff = if ctx.tool.nose == ToolNose::Ball { ae.min(d_eff) } else { ae };
        thinning = if ae_eff < d_eff / 2.0 {
            let phis = (1.0 - 2.0 * ae_eff / d_eff.max(1e-6)).clamp(-1.0, 1.0).acos();
            (1.0 / (phis.sin() * sin_k.max(0.2))).min(2.5)
        } else {
            1.0
        };
        fz = fz_target * fz_scale * thinning;
        let mut feed = rpm * z * fz;
        if feed > ctx.machine.max_feed_mm_min {
            feed = ctx.machine.max_feed_mm_min;
            fz = feed / (rpm * z);
        }
        conds = CuttingConditions {
            cutting_speed_m_min: PI * d * rpm / 1000.0,
            feed_rate_mm_min: feed,
            feed_per_tooth_mm: fz,
            axial_doc_mm: ap,
            radial_doc_mm: ae,
            spindle_rpm: rpm.round() as u32,
            coolant: match ctx.coolant.method {
                CoolantMethod::Flood => CoolantType::Flood,
                CoolantMethod::Mist => CoolantType::Mist,
                CoolantMethod::AirBlast => CoolantType::AirBlast,
                CoolantMethod::Dry => CoolantType::Dry,
                CoolantMethod::ThroughTool => CoolantType::ThroughTool,
            },
        };
        let a = analyze_cut(ctx, &conds, true, body);
        let power_ok = a.spindle_power_kw <= 0.9 * a.available_power_kw;
        let defl_ok = a.wall.deflection_um.abs() <= budget_um;
        last = a;
        if (power_ok && defl_ok) || iter >= 16 {
            if !power_ok {
                notes.push("동력 한계 내 조건을 찾지 못했습니다. 공구·기계 사양을 확인하세요.".into());
            }
            if !defl_ok {
                notes.push(format!(
                    "휨 예산 {:.1} µm 를 만족하지 못했습니다 (예측 {:.1} µm). 돌출 길이 단축 또는 공구 직경 상향을 검토하세요.",
                    budget_um,
                    last.wall.deflection_um.abs()
                ));
            }
            break;
        }
        if !power_ok {
            if ap > 0.3 * d && purpose == Purpose::Roughing {
                ap *= 0.8;
            } else {
                ae *= 0.85;
            }
        } else if fz_scale > 0.5 {
            fz_scale *= 0.9;
        } else if purpose == Purpose::Roughing && ae > 0.05 * d {
            ae *= 0.85;
        } else if ap > 0.2 * d {
            ap *= 0.85;
        } else {
            fz_scale *= 0.9;
        }
        iter += 1;
    }
    if iter > 0 {
        notes.push(format!(
            "제약 조정 {}회: ap {:.2} mm, ae {:.2} mm, fz 배율 {:.2}.",
            iter, ap, ae, fz_scale
        ));
    }
    Recommendation {
        purpose,
        predicted_wall_error_um: last.wall.deflection_um,
        predicted_tool_life_min: last.wear.tool_life_min,
        conditions: conds,
        fz_target_mm: fz_target,
        chip_thinning_factor: thinning,
        deflection_budget_um: budget_um,
        notes,
    }
}

pub fn flute_count_from_conditions(conds: &CuttingConditions) -> u32 {
    if conds.spindle_rpm == 0 || conds.feed_per_tooth_mm <= 0.0 {
        return 4;
    }
    (conds.feed_rate_mm_min / (conds.spindle_rpm as f64 * conds.feed_per_tooth_mm)).round().clamp(1.0, 12.0) as u32
}