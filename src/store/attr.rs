use crate::cutting::WorkpieceMaterial;
use crate::physics::{tribology_from_name, WorkpieceProps};
use crate::profile::{EndMillMockupSetting, ToolNose};
use crate::workpiece_setup::{ClampingMethod, StockShape, WorkpieceSetup};
use serde::{Deserialize, Serialize};

pub const ENDMILL_DIM: usize = 20;
pub const WORKPIECE_DIM: usize = 20;
pub const ATTR_VECTOR_VERSION: &str = "2-tribology";

fn q(v: f64, step: f64) -> f64 {
    (v / step).round() * step
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EndMillAttr {
    pub diameter_mm: f64,
    pub flutes: u8,
    pub loc_mm: f64,
    pub oal_mm: f64,
    pub shank_mm: f64,
    pub helix_deg: f64,
    pub nose: String,
    pub corner_r_mm: f64,
    pub coating_family: String,
    pub variable_pitch: bool,
    pub substrate: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EndMillAlias {
    pub name: String,
    pub model: String,
    pub high_end: bool,
    pub coating_name: String,
}

impl EndMillAttr {
    pub fn from_setting(s: &EndMillMockupSetting) -> Self {
        let d = q(s.diameter_mm, 0.01);
        let (nose, cr) = match s.nose {
            ToolNose::Square => ("square", 0.0),
            ToolNose::Ball => ("ball", d / 2.0),
            ToolNose::CornerRadius { radius_mm } => ("corner", q(radius_mm.min(d / 2.0).max(0.0), 0.01)),
        };
        Self {
            diameter_mm: d,
            flutes: s.flute_count.max(1),
            loc_mm: q(s.loc_mm, 0.1),
            oal_mm: q(s.oal_mm, 0.1),
            shank_mm: q(s.shank_diameter_mm, 0.1),
            helix_deg: q(s.helix_angle_deg, 1.0),
            nose: nose.into(),
            corner_r_mm: cr,
            coating_family: tribology_from_name(s.coating_name.as_deref()).family,
            variable_pitch: s.variable_pitch,
            substrate: s.substrate.key().into(),
            rake_deg: s.rake_deg.map(|v| q(v, 0.5)),
            clearance_deg: s.clearance_deg.map(|v| q(v, 0.5)),
            edge_radius_um: s.edge_radius_um.map(|v| q(v, 0.5)),
            neck_diameter_mm: s.neck().map(|(d, _)| q(d, 0.01)),
            reach_mm: s.neck().map(|(_, r)| q(r, 0.1)),
        }
    }

    pub fn alias_of(s: &EndMillMockupSetting) -> EndMillAlias {
        EndMillAlias {
            name: s.name.trim().to_string(),
            model: s.model.trim().to_string(),
            high_end: s.is_high_end,
            coating_name: s.coating_name.clone().unwrap_or_default(),
        }
    }

    pub fn key(&self) -> String {
        let nose = match self.nose.as_str() {
            "corner" => format!("cr{:.2}", self.corner_r_mm),
            other => other.to_string(),
        };
        format!(
            "em:d{:.2}:z{}:loc{:.1}:oal{:.1}:sh{:.1}:hx{:.0}:{}:c-{}:vp{}:{}",
            self.diameter_mm,
            self.flutes,
            self.loc_mm,
            self.oal_mm,
            self.shank_mm,
            self.helix_deg,
            nose,
            self.coating_family.to_lowercase(),
            u8::from(self.variable_pitch),
            self.substrate
        ) + &self.extra_key()
    }

    pub fn extra_key(&self) -> String {
        let mut k = String::new();
        if let Some(v) = self.rake_deg {
            k.push_str(&format!(":rk{:.1}", v));
        }
        if let Some(v) = self.clearance_deg {
            k.push_str(&format!(":cl{:.1}", v));
        }
        if let Some(v) = self.edge_radius_um {
            k.push_str(&format!(":er{:.1}", v));
        }
        if let (Some(d), Some(r)) = (self.neck_diameter_mm, self.reach_mm) {
            k.push_str(&format!(":nk{:.2}x{:.1}", d, r));
        }
        k
    }

    pub fn label(&self) -> String {
        let nose = match self.nose.as_str() {
            "ball" => "볼".to_string(),
            "corner" => format!("코너R{:.2}", self.corner_r_mm),
            _ => "스퀘어".to_string(),
        };
        let substrate = crate::profile::ToolSubstrate::from_key(&self.substrate)
            .filter(|t| *t != crate::profile::ToolSubstrate::Carbide)
            .map(|t| format!(" {}", t.label()))
            .unwrap_or_default();
        let neck = match (self.neck_diameter_mm, self.reach_mm) {
            (Some(d), Some(r)) => format!(" 넥Ø{:.2}×{:.0}", d, r),
            _ => String::new(),
        };
        format!(
            "Ø{:.2} {}날 {} LOC{:.0} H{:.0}° {}{}{}{}",
            self.diameter_mm,
            self.flutes,
            nose,
            self.loc_mm,
            self.helix_deg,
            self.coating_family,
            if self.variable_pitch { " 부등분할" } else { "" },
            neck,
            substrate
        )
    }

    pub fn to_setting(&self, alias: Option<&EndMillAlias>) -> EndMillMockupSetting {
        let nose = match self.nose.as_str() {
            "ball" => ToolNose::Ball,
            "corner" => ToolNose::CornerRadius { radius_mm: self.corner_r_mm },
            _ => ToolNose::Square,
        };
        let (name, model, high_end, coating) = match alias {
            Some(a) => (
                a.name.clone(),
                a.model.clone(),
                a.high_end,
                if a.coating_name.trim().is_empty() { self.coating_family.clone() } else { a.coating_name.clone() },
            ),
            None => (self.label(), self.key(), true, self.coating_family.clone()),
        };
        EndMillMockupSetting {
            name,
            model,
            diameter_mm: self.diameter_mm,
            flute_count: self.flutes,
            loc_mm: self.loc_mm,
            oal_mm: self.oal_mm.max(self.loc_mm),
            shank_diameter_mm: self.shank_mm,
            helix_angle_deg: self.helix_deg,
            coating_name: if coating == "uncoated" { None } else { Some(coating) },
            is_high_end: high_end,
            nose,
            stickout_mm: None,
            runout_um: None,
            variable_pitch: self.variable_pitch,
            substrate: crate::profile::ToolSubstrate::from_key(&self.substrate).unwrap_or_default(),
            rake_deg: self.rake_deg,
            clearance_deg: self.clearance_deg,
            edge_radius_um: self.edge_radius_um,
            neck_diameter_mm: self.neck_diameter_mm,
            reach_mm: self.reach_mm,
        }
    }

    pub fn vector(&self) -> Vec<f32> {
        let d = self.diameter_mm.max(0.1);
        let tri = tribology_from_name(Some(&self.coating_family));
        let substrate = match self.substrate.as_str() {
            "hss" => -1.0,
            "hss_co" => -0.8,
            "carbide_uf" => 0.1,
            "cbn" => 1.0,
            "pcd" => 1.5,
            _ => 0.0,
        };
        let v = [
            (d / 10.0).ln() * 1.5,
            (self.flutes as f64 - 4.0) / 2.0 * 0.8,
            ((self.loc_mm / d).max(0.1) / 2.5).ln() * 0.8,
            ((self.oal_mm / d).max(0.5) / 7.0).ln() * 0.4,
            (self.shank_mm / d - 1.0) * 0.6,
            (self.helix_deg - 35.0) / 15.0 * 0.6,
            if self.nose == "square" { 0.7 } else { 0.0 },
            if self.nose == "ball" { 0.7 } else { 0.0 },
            if self.nose == "corner" { 0.7 } else { 0.0 },
            self.corner_r_mm / d * 1.0,
            (tri.friction - 0.45) / 0.15 * 0.4,
            (tri.max_temp_c - 900.0) / 300.0 * 0.4,
            (tri.wear_factor - 0.4) / 0.3 * 0.4,
            if tri.ferrous_ok { 0.0 } else { 0.5 },
            if self.variable_pitch { 0.3 } else { 0.0 },
            substrate * 0.5,
            if tri.chem.al { 0.4 } else { 0.0 },
            if tri.chem.carbon { 0.5 } else { 0.0 },
            (tri.oxidation_c - 900.0) / 300.0 * 0.4,
            (tri.hardness_hv.max(500.0) / 3500.0).ln() * 0.4,
        ];
        v.iter().map(|x| *x as f32).collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttrDiff {
    pub attribute: String,
    pub a: String,
    pub b: String,
    pub share: f64,
}

pub fn endmill_diff(a: &EndMillAttr, b: &EndMillAttr) -> Vec<AttrDiff> {
    let va = a.vector();
    let vb = b.vector();
    let names = [
        "직경",
        "날 수",
        "유효장/직경",
        "전장/직경",
        "샹크/직경",
        "헬릭스",
        "스퀘어",
        "볼",
        "코너R",
        "코너R/직경",
        "코팅 마찰",
        "코팅 내열",
        "코팅 마모계수",
        "비철 전용 코팅",
        "부등분할",
        "모재",
        "코팅 Al 함유 (Al 응착 친화)",
        "탄소계 코팅 (Fe·Ni·Ti 확산)",
        "코팅 산화 개시 온도",
        "코팅 경도",
    ];
    let shown = |i: usize, x: &EndMillAttr| -> String {
        match i {
            0 => format!("{:.2} mm", x.diameter_mm),
            1 => format!("{}", x.flutes),
            2 => format!("{:.1} mm ({:.1}D)", x.loc_mm, x.loc_mm / x.diameter_mm.max(0.1)),
            3 => format!("{:.1} mm", x.oal_mm),
            4 => format!("{:.1} mm", x.shank_mm),
            5 => format!("{:.0}°", x.helix_deg),
            6..=8 => x.nose.clone(),
            9 => format!("{:.2} mm", x.corner_r_mm),
            10..=13 | 16..=19 => x.coating_family.clone(),
            14 => if x.variable_pitch { "예".into() } else { "아니오".into() },
            _ => x.substrate.clone(),
        }
    };
    let sq: Vec<f64> = va.iter().zip(vb.iter()).map(|(x, y)| ((x - y) as f64).powi(2)).collect();
    let total: f64 = sq.iter().sum::<f64>().max(1e-12);
    let mut out: Vec<AttrDiff> = sq
        .iter()
        .enumerate()
        .filter(|(_, s)| **s > 1e-9)
        .map(|(i, s)| AttrDiff {
            attribute: names[i].into(),
            a: shown(i, a),
            b: shown(i, b),
            share: s / total,
        })
        .collect();
    out.sort_by(|x, y| y.share.partial_cmp(&x.share).unwrap_or(std::cmp::Ordering::Equal));
    out
}

pub fn similarity_from_l2(sq_dist: f32) -> f64 {
    (-(sq_dist.max(0.0) as f64) / 2.0).exp()
}

pub fn l2_sq(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y) * (x - y)).sum()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkpieceAttr {
    pub material_key: String,
    pub material_family: String,
    pub hardness_hrc: Option<f64>,
    pub shape: String,
    pub width_mm: f64,
    pub height_mm: f64,
    pub thickness_mm: f64,
    pub clamping: String,
    pub tolerance_mm: f64,
    pub ra_target_um: f64,
    pub pre_machined: bool,
}

pub fn clamping_key(c: &ClampingMethod) -> &'static str {
    match c {
        ClampingMethod::Vise => "vise",
        ClampingMethod::VacuumChuck => "vacuum",
        ClampingMethod::MagneticChuck => "magnetic",
        ClampingMethod::FixturePlate => "fixture",
        ClampingMethod::SoftJaw => "soft_jaw",
    }
}

pub fn clamping_from_key(k: &str) -> ClampingMethod {
    match k {
        "vacuum" => ClampingMethod::VacuumChuck,
        "magnetic" => ClampingMethod::MagneticChuck,
        "fixture" => ClampingMethod::FixturePlate,
        "soft_jaw" => ClampingMethod::SoftJaw,
        _ => ClampingMethod::Vise,
    }
}

pub fn clamping_stiffness_n_per_um(c: &ClampingMethod) -> f64 {
    c.stiffness_n_per_um()
}

impl WorkpieceAttr {
    pub fn from_setup(w: &WorkpieceSetup) -> Self {
        let m = w.effective_material();
        let hardness = match &m {
            WorkpieceMaterial::AlloySteel { hardness_hrc } => Some(*hardness_hrc as f64),
            _ => w.hardness_hrc.map(|h| h as f64),
        };
        let shape = match &w.shape {
            StockShape::Rectangular => "rect".to_string(),
            StockShape::Cylindrical { diameter_mm } => format!("cyl{:.0}", diameter_mm),
            StockShape::Custom => "custom".to_string(),
        };
        Self {
            material_key: m.key(),
            material_family: m.family_key().to_string(),
            hardness_hrc: hardness,
            shape,
            width_mm: q(w.width_mm, 1.0),
            height_mm: q(w.height_mm, 1.0),
            thickness_mm: q(w.thickness_mm, 0.5),
            clamping: clamping_key(&w.clamping).into(),
            tolerance_mm: q(w.effective_tolerance_mm(), 0.001),
            ra_target_um: q(w.surface_roughness_target_ra, 0.1),
            pre_machined: w.pre_machined,
        }
    }

    pub fn key(&self) -> String {
        format!(
            "wp:{}:{}:{:.0}x{:.0}x{:.1}:{}:tol{:.3}:ra{:.1}:pm{}",
            self.material_key,
            self.shape,
            self.width_mm,
            self.height_mm,
            self.thickness_mm,
            self.clamping,
            self.tolerance_mm,
            self.ra_target_um,
            u8::from(self.pre_machined)
        )
    }

    pub fn props(&self, setup: &WorkpieceSetup) -> WorkpieceProps {
        WorkpieceProps::of(&setup.effective_material())
    }

    pub fn label(&self, setup: &WorkpieceSetup) -> String {
        format!(
            "{} {:.0}×{:.0}×{:.1} {} 공차 {:.3}",
            setup.material_label(),
            self.width_mm,
            self.height_mm,
            self.thickness_mm,
            self.clamping,
            self.tolerance_mm
        )
    }

    pub fn vector(&self, setup: &WorkpieceSetup) -> Vec<f32> {
        let p = WorkpieceProps::of(&setup.effective_material());
        let size = self.width_mm.max(self.height_mm).max(1.0);
        let slender = (self.thickness_mm.max(0.5) / size / 0.25).max(1e-3);
        let clamp = clamping_stiffness_n_per_um(&clamping_from_key(&self.clamping));
        let v = [
            (p.kc11 / 1600.0).ln() * 1.2,
            (p.mc - 0.25) / 0.05 * 0.3,
            (self.hardness_hrc.unwrap_or(p.hardness_hrc) - 25.0) / 15.0 * 0.6,
            (p.conductivity / 50.0).ln() * 0.6,
            (p.density / 7850.0).ln() * 0.4,
            (p.elastic_gpa / 205.0).ln() * 0.5,
            (p.adhesion - 1.0) / 0.2 * 0.4,
            (p.thermal_factor - 1.0) / 0.3 * 0.3,
            (p.melt_c / 1450.0).ln() * 0.3,
            (size / 100.0).ln() * 0.4,
            slender.ln() * 0.5,
            (clamp.ln() - 80f64.ln()) * 0.4,
            if self.shape.starts_with("cyl") { 0.3 } else { 0.0 },
            (self.tolerance_mm.max(0.001) / 0.05).ln() * 0.5,
            (self.ra_target_um.max(0.05) / 1.6).ln() * 0.4,
            if self.pre_machined { 0.2 } else { 0.0 },
            if p.chem.reactive() { 0.4 } else { 0.0 },
            (crate::tribology::workpiece_abrasiveness(&p).max(1e-4) / 0.005).ln() * 0.25,
            (p.k_at(500.0) / p.k_at(20.0)).ln() * 0.6,
            (p.wear_activation_k - 7000.0) / 2000.0 * 0.3,
        ];
        v.iter().map(|x| *x as f32).collect()
    }
}

pub fn workpiece_diff(a: &WorkpieceAttr, sa: &WorkpieceSetup, b: &WorkpieceAttr, sb: &WorkpieceSetup) -> Vec<AttrDiff> {
    let va = a.vector(sa);
    let vb = b.vector(sb);
    let names = [
        "비절삭저항 kc1.1",
        "Kienzle mc",
        "경도",
        "열전도",
        "밀도",
        "탄성계수",
        "응착성",
        "열 계수",
        "융점",
        "크기",
        "두께비",
        "클램핑 강성",
        "형상",
        "공차",
        "목표 Ra",
        "선가공",
        "화학 반응성 (Ni·Ti 계)",
        "연삭성 (경질상)",
        "고온 열전도 변화",
        "확산 활성화 온도",
    ];
    let sq: Vec<f64> = va.iter().zip(vb.iter()).map(|(x, y)| ((x - y) as f64).powi(2)).collect();
    let total: f64 = sq.iter().sum::<f64>().max(1e-12);
    let show = |i: usize, x: &WorkpieceAttr, s: &WorkpieceSetup| -> String {
        match i {
            0..=8 | 16..=19 => s.material_label(),
            9 => format!("{:.0}×{:.0}", x.width_mm, x.height_mm),
            10 => format!("{:.1} mm", x.thickness_mm),
            11 => x.clamping.clone(),
            12 => x.shape.clone(),
            13 => format!("{:.3} mm", x.tolerance_mm),
            14 => format!("{:.1} µm", x.ra_target_um),
            _ => if x.pre_machined { "예".into() } else { "아니오".into() },
        }
    };
    let mut out: Vec<AttrDiff> = sq
        .iter()
        .enumerate()
        .filter(|(_, s)| **s > 1e-9)
        .map(|(i, s)| AttrDiff {
            attribute: names[i].into(),
            a: show(i, a, sa),
            b: show(i, b, sb),
            share: s / total,
        })
        .collect();
    out.sort_by(|x, y| y.share.partial_cmp(&x.share).unwrap_or(std::cmp::Ordering::Equal));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_does_not_change_key() {
        let mut a = EndMillMockupSetting::default_10mm_4flute();
        let mut b = a.clone();
        b.name = "다른 브랜드 Ø10".into();
        b.model = "OTHER-4F".into();
        b.is_high_end = false;
        assert_eq!(EndMillAttr::from_setting(&a).key(), EndMillAttr::from_setting(&b).key());
        a.helix_angle_deg = 45.0;
        assert_ne!(EndMillAttr::from_setting(&a).key(), EndMillAttr::from_setting(&b).key());
    }

    #[test]
    fn coating_marketing_name_maps_to_family() {
        let mut a = EndMillMockupSetting::default_10mm_4flute();
        a.coating_name = Some("nACo HiPIMS".into());
        let mut b = a.clone();
        b.coating_name = Some("NACO".into());
        assert_eq!(EndMillAttr::from_setting(&a).key(), EndMillAttr::from_setting(&b).key());
    }

    #[test]
    fn nearer_tool_has_smaller_distance() {
        let base = EndMillAttr::from_setting(&EndMillMockupSetting::default_10mm_4flute());
        let mut near = EndMillMockupSetting::default_10mm_4flute();
        near.diameter_mm = 10.5;
        let far = EndMillMockupSetting::default_6mm_2flute_aluminum();
        let dn = l2_sq(&base.vector(), &EndMillAttr::from_setting(&near).vector());
        let df = l2_sq(&base.vector(), &EndMillAttr::from_setting(&far).vector());
        assert!(dn < df);
        let diff = endmill_diff(&base, &EndMillAttr::from_setting(&far));
        assert!(!diff.is_empty());
        assert!((diff.iter().map(|d| d.share).sum::<f64>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn workpiece_vector_separates_materials() {
        let al = WorkpieceSetup::default_aluminum_plate();
        let st = WorkpieceSetup::default_steel_block();
        let mut st2 = st.clone();
        st2.width_mm = 160.0;
        let a = WorkpieceAttr::from_setup(&al);
        let s = WorkpieceAttr::from_setup(&st);
        let s2 = WorkpieceAttr::from_setup(&st2);
        assert_ne!(s.key(), s2.key());
        assert!(l2_sq(&s.vector(&st), &s2.vector(&st2)) < l2_sq(&s.vector(&st), &a.vector(&al)));
    }
}
