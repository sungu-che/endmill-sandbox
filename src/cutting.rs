use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 피삭재 (가공 대상 소재)
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkpieceMaterial {
    /// 알루미늄 합금
    Aluminum,
    /// 일반 탄소강 (S45C 등)
    CarbonSteel,
    /// 합금강 / 금형강 (SKD11, HRC 50~60)
    AlloySteel { hardness_hrc: u8 },
    /// 스테인리스강
    StainlessSteel,
    /// 티타늄 합금 (Ti-6Al-4V 등)
    Titanium,
    /// 인코넬 / 니켈 초합금 (항공용)
    Inconel,
    /// 수퍼알로이
    SuperAlloy,
    /// CFRP (탄소섬유 강화 플라스틱)
    CFRP,
}

impl WorkpieceMaterial {
    pub fn reference_cutting_speed(&self) -> f64 {
        match self {
            Self::Aluminum => 300.0,
            Self::CarbonSteel => 180.0,
            Self::AlloySteel { hardness_hrc } => match hardness_hrc {
                0..=30 => 150.0,
                31..=45 => 110.0,
                46..=55 => 90.0,
                _ => 70.0,
            },
            Self::StainlessSteel => 120.0,
            Self::Titanium => 60.0,
            Self::Inconel => 30.0,
            Self::SuperAlloy => 25.0,
            Self::CFRP => 200.0,
        }
    }

    pub fn is_hard_to_machine(&self) -> bool {
        matches!(
            self,
            Self::Titanium | Self::Inconel | Self::SuperAlloy
                | Self::AlloySteel { hardness_hrc: 50.. }
        )
    }

    pub fn kienzle_constants(&self) -> (f64, f64) {
        let p = crate::physics::WorkpieceProps::of(self);
        (p.kc11, p.mc)
    }

    pub fn key(&self) -> String {
        match self {
            Self::Aluminum => "aluminum".into(),
            Self::CarbonSteel => "carbon_steel".into(),
            Self::AlloySteel { hardness_hrc } => format!("alloy_steel_hrc{}", hardness_hrc),
            Self::StainlessSteel => "stainless".into(),
            Self::Titanium => "titanium".into(),
            Self::Inconel => "inconel".into(),
            Self::SuperAlloy => "superalloy".into(),
            Self::CFRP => "cfrp".into(),
        }
    }

    pub fn family_key(&self) -> &'static str {
        match self {
            Self::Aluminum => "aluminum",
            Self::CarbonSteel => "carbon_steel",
            Self::AlloySteel { .. } => "alloy_steel",
            Self::StainlessSteel => "stainless",
            Self::Titanium => "titanium",
            Self::Inconel => "inconel",
            Self::SuperAlloy => "superalloy",
            Self::CFRP => "cfrp",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysicalSimulationResult {
    pub mrr_cm3_min: f64,
    pub cutting_force_n: f64,
    pub spindle_power_kw: f64,
    pub tool_deflection_mm: f64,
    #[serde(default)]
    pub torque_nm: f64,
    #[serde(default)]
    pub chip_thickness_max_mm: f64,
    #[serde(default)]
    pub interface_temp_c: f64,
    #[serde(default)]
    pub tool_life_min: f64,
    #[serde(default)]
    pub wall_error_um: f64,
}

/// 절삭 조건 파라미터
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuttingConditions {
    /// 절삭 속도 (m/min)
    pub cutting_speed_m_min: f64,
    /// 이송 속도 (mm/min)
    pub feed_rate_mm_min: f64,
    /// 칩당 이송량 (mm/tooth)
    pub feed_per_tooth_mm: f64,
    /// 축방향 절입 깊이 (mm) - Axial Depth of Cut
    pub axial_doc_mm: f64,
    /// 반경방향 절입 깊이 (mm) - Radial Depth of Cut
    pub radial_doc_mm: f64,
    /// 스핀들 회전수 (RPM)
    pub spindle_rpm: u32,
    /// 냉각 방식
    pub coolant: CoolantType,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoolantType {
    /// 수용성 냉각수 (Flood)
    Flood,
    /// 미스트 (MQL)
    Mist,
    /// 에어 블로우
    AirBlast,
    /// 건식 (Dry)
    Dry,
    /// 내부 급유 (Through-tool)
    ThroughTool,
}

/// 절삭 조건 계산 에러
#[derive(Error, Debug)]
pub enum CuttingCalcError {
    #[error("공구 직경은 0보다 커야 합니다: {0}")]
    InvalidDiameter(f64),
    #[error("절삭 속도가 소재 허용 범위를 초과합니다: {actual} > {max}")]
    SpeedExceedsLimit { actual: f64, max: f64 },
    #[error("이송량이 칩 배출 한계를 초과합니다")]
    FeedExceedsChipEvacuation,
    #[error("절입 깊이가 공구 유효 길이를 초과합니다")]
    DocExceedsEffectiveLength,
}

/// 절삭 조건 계산기 (Speeds & Feeds Calculator)
///
/// 대화에서 언급된 FSWizard, G-Wizard, CAM 내장 계산기의 로직을 모델링.
/// "이론값"을 제공하며, 현장에서는 ±10~20% 미세 조정.
pub struct CuttingCalculator;

impl CuttingCalculator {
    /// 스핀들 RPM 계산
    ///
    /// RPM = (절삭속도 × 1000) / (π × 공구직경)
    pub fn calc_spindle_rpm(
        cutting_speed_m_min: f64,
        tool_diameter_mm: f64,
    ) -> Result<u32, CuttingCalcError> {
        if tool_diameter_mm <= 0.0 {
            return Err(CuttingCalcError::InvalidDiameter(tool_diameter_mm));
        }
        let rpm = (cutting_speed_m_min * 1000.0) / (std::f64::consts::PI * tool_diameter_mm);
        Ok(rpm.round() as u32)
    }

    /// 이송 속도 계산
    ///
    /// 이송속도(mm/min) = RPM × 날수 × 칩당 이송량(mm/tooth)
    pub fn calc_feed_rate(
        spindle_rpm: u32,
        flute_count: u8,
        feed_per_tooth_mm: f64,
    ) -> f64 {
        spindle_rpm as f64 * flute_count as f64 * feed_per_tooth_mm
    }

    pub fn recommend_conditions(
        workpiece: &WorkpieceMaterial,
        tool_diameter_mm: f64,
        flute_count: u8,
        is_high_end_tool: bool,
    ) -> Result<CuttingConditions, CuttingCalcError> {
        if tool_diameter_mm <= 0.0 {
            return Err(CuttingCalcError::InvalidDiameter(tool_diameter_mm));
        }

        if flute_count == 0 {
            return Err(CuttingCalcError::FeedExceedsChipEvacuation);
        }
        let props = crate::physics::WorkpieceProps::of(workpiece);
        let speed_factor = if is_high_end_tool { 1.0 } else { 0.55 };
        let cutting_speed = props.ref_vc * speed_factor;
        let rpm = Self::calc_spindle_rpm(cutting_speed, tool_diameter_mm)?;
        let fz_target = (props.fz_coeff * tool_diameter_mm).clamp(0.003, 0.25)
            * if is_high_end_tool { 1.0 } else { 0.8 };
        let axial_doc = props.ap_ratio * tool_diameter_mm;
        let radial_doc = props.ae_ratio * tool_diameter_mm;
        let thinning = if radial_doc < tool_diameter_mm / 2.0 {
            let phis = (1.0 - 2.0 * radial_doc / tool_diameter_mm).clamp(-1.0, 1.0).acos();
            (1.0 / phis.sin()).min(2.5)
        } else {
            1.0
        };
        let feed_per_tooth = fz_target * thinning;
        let feed_rate = Self::calc_feed_rate(rpm, flute_count, feed_per_tooth);
        let coolant = match workpiece {
            WorkpieceMaterial::AlloySteel { hardness_hrc } if *hardness_hrc >= 45 => CoolantType::AirBlast,
            WorkpieceMaterial::CFRP => CoolantType::AirBlast,
            WorkpieceMaterial::Aluminum => CoolantType::Flood,
            m if m.is_hard_to_machine() => CoolantType::ThroughTool,
            _ => CoolantType::Mist,
        };
        Ok(CuttingConditions {
            cutting_speed_m_min: Self::actual_cutting_speed(rpm, tool_diameter_mm),
            feed_rate_mm_min: feed_rate,
            feed_per_tooth_mm: feed_per_tooth,
            axial_doc_mm: axial_doc,
            radial_doc_mm: radial_doc,
            spindle_rpm: rpm,
            coolant,
        })
    }

    pub fn actual_cutting_speed(spindle_rpm: u32, tool_diameter_mm: f64) -> f64 {
        std::f64::consts::PI * tool_diameter_mm * spindle_rpm as f64 / 1000.0
    }

    pub fn apply_low_end_derating(
        conditions: &CuttingConditions,
        derate_factor: f64,
    ) -> CuttingConditions {
        let factor = derate_factor.clamp(0.2, 0.6);
        let new_rpm = (conditions.spindle_rpm as f64 * factor).round().max(1.0) as u32;
        let flutes = crate::physics::flute_count_from_conditions(conditions) as f64;
        let rpm_ratio = new_rpm as f64 / conditions.spindle_rpm.max(1) as f64;
        CuttingConditions {
            cutting_speed_m_min: conditions.cutting_speed_m_min * rpm_ratio,
            feed_rate_mm_min: new_rpm as f64 * flutes * conditions.feed_per_tooth_mm,
            feed_per_tooth_mm: conditions.feed_per_tooth_mm,
            axial_doc_mm: conditions.axial_doc_mm * 0.5,
            radial_doc_mm: conditions.radial_doc_mm * 0.5,
            spindle_rpm: new_rpm,
            coolant: conditions.coolant.clone(),
        }
    }

    pub fn simulate_physics(
        workpiece: &WorkpieceMaterial,
        conds: &CuttingConditions,
        tool_diameter_mm: f64,
        tool_loc_mm: f64,
    ) -> PhysicalSimulationResult {
        let flutes = crate::physics::flute_count_from_conditions(conds) as u8;
        let setting = crate::profile::EndMillMockupSetting::custom(
            "simulate".into(),
            "simulate".into(),
            tool_diameter_mm,
            flutes,
            tool_loc_mm,
            tool_loc_mm + 3.0 * tool_diameter_mm,
            tool_diameter_mm,
            35.0,
            Some("AlTiN".into()),
            true,
        );
        let props = crate::physics::WorkpieceProps::of(workpiece);
        let machine = crate::profile::MachineLimits::default();
        let tool = crate::physics::ToolGeometry::from_setting(&setting, &props, &machine, None);
        let ctx = crate::physics::CutContext {
            coolant: crate::physics::CoolantState::from_type(&conds.coolant),
            machine,
            calib: crate::physics::Calibration::default(),
            tolerance_mm: 0.02,
            allowance_mm: 0.5,
            fixture_stiffness_n_per_um: crate::workpiece_setup::ClampingMethod::Vise.stiffness_n_per_um(),
            wp: props,
            tool,
        };
        let body = crate::physics::ThermalBody {
            mass_kg: 2.0,
            area_m2: 0.04,
            size_mm: 100.0,
        };
        let a = crate::physics::analyze_cut(&ctx, conds, true, body);
        PhysicalSimulationResult {
            mrr_cm3_min: a.mrr_cm3_min,
            cutting_force_n: a.forces.f_res_peak,
            spindle_power_kw: a.spindle_power_kw,
            tool_deflection_mm: a.deflection_peak_um / 1000.0,
            torque_nm: a.forces.torque_mean_nm,
            chip_thickness_max_mm: a.forces.h_max_mm,
            interface_temp_c: a.thermal.interface_c,
            tool_life_min: a.wear.tool_life_min,
            wall_error_um: a.wall.deflection_um,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rpm_calculation() {
        // D=10mm, Vc=200 m/min → RPM ≈ 6366
        let rpm = CuttingCalculator::calc_spindle_rpm(200.0, 10.0).unwrap();
        assert_eq!(rpm, 6366);
    }

    #[test]
    fn test_recommend_for_titanium() {
        let conds = CuttingCalculator::recommend_conditions(
            &WorkpieceMaterial::Titanium,
            10.0,
            4,
            true,
        )
        .unwrap();

        // 티타늄: 기준 60 m/min
        assert!((conds.cutting_speed_m_min - 60.0).abs() < 0.1);
        assert!(conds.axial_doc_mm <= 10.0);
    }

    #[test]
    fn test_low_end_derating() {
        let base = CuttingConditions {
            cutting_speed_m_min: 200.0,
            feed_rate_mm_min: 2000.0,
            feed_per_tooth_mm: 0.1,
            axial_doc_mm: 10.0,
            radial_doc_mm: 3.0,
            spindle_rpm: 6366,
            coolant: CoolantType::Flood,
        };

        let derated = CuttingCalculator::apply_low_end_derating(&base, 0.4);
        assert!(derated.spindle_rpm < base.spindle_rpm);
        assert!(derated.axial_doc_mm < base.axial_doc_mm);
    }
}