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
                31..=45 => 100.0,
                46..=55 => 60.0,
                _ => 40.0,
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
        match self {
            Self::Aluminum => (700.0, 0.25),
            Self::CarbonSteel => (1500.0, 0.22),
            Self::AlloySteel { .. } => (1900.0, 0.22),
            Self::StainlessSteel => (1800.0, 0.23),
            Self::Titanium => (1300.0, 0.23),
            Self::Inconel => (2400.0, 0.24),
            Self::SuperAlloy => (2500.0, 0.24),
            Self::CFRP => (600.0, 0.20),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysicalSimulationResult {
    pub mrr_cm3_min: f64,
    pub cutting_force_n: f64,
    pub spindle_power_kw: f64,
    pub tool_deflection_mm: f64,
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

    /// 피삭재 + 공구 직경 기반 권장 절삭 조건 산출
    pub fn recommend_conditions(
        workpiece: &WorkpieceMaterial,
        tool_diameter_mm: f64,
        flute_count: u8,
        is_high_end_tool: bool,
    ) -> Result<CuttingConditions, CuttingCalcError> {
        if tool_diameter_mm <= 0.0 {
            return Err(CuttingCalcError::InvalidDiameter(tool_diameter_mm));
        }

        // 고급 공구는 기준 속도의 100%, 저가는 50~60% 적용
        let speed_factor = if is_high_end_tool { 1.0 } else { 0.55 };
        let base_speed = workpiece.reference_cutting_speed();
        let cutting_speed = base_speed * speed_factor;

        let rpm = Self::calc_spindle_rpm(cutting_speed, tool_diameter_mm)?;

        // 칩당 이송량: 직경 비례 (간이 공식)
        // 일반적: D × 0.02 ~ 0.05 (mm/tooth)
        let fpt_base = tool_diameter_mm * 0.03;
        let feed_per_tooth = if workpiece.is_hard_to_machine() {
            fpt_base * 0.5 // 난삭재는 이송 감소
        } else {
            fpt_base
        };

        let feed_rate = Self::calc_feed_rate(rpm, flute_count, feed_per_tooth);

        // 절입 깊이: 황삭 기준 축방향 = 직경 × 1.0, 반경 = 직경 × 0.3
        let axial_doc = tool_diameter_mm * 1.0;
        let radial_doc = tool_diameter_mm * 0.3;

        let coolant = if workpiece.is_hard_to_machine() {
            CoolantType::ThroughTool
        } else if matches!(workpiece, WorkpieceMaterial::Aluminum) {
            CoolantType::Flood
        } else {
            CoolantType::Mist
        };

        Ok(CuttingConditions {
            cutting_speed_m_min: cutting_speed,
            feed_rate_mm_min: feed_rate,
            feed_per_tooth_mm: feed_per_tooth,
            axial_doc_mm: axial_doc,
            radial_doc_mm: radial_doc,
            spindle_rpm: rpm,
            coolant,
        })
    }

    /// 저가 공구로 고급 공구 조건 "흉내" 시 감속 계수 적용
    ///
    /// 대화 내용: "절삭 속도를 30~50% 낮추고, 절입 깊이를 최소화"
    pub fn apply_low_end_derating(
        conditions: &CuttingConditions,
        derate_factor: f64, // 0.3 ~ 0.5
    ) -> CuttingConditions {
        let factor = derate_factor.clamp(0.2, 0.6);
        let new_rpm = (conditions.spindle_rpm as f64 * factor).round() as u32;
        let new_feed = conditions.feed_rate_mm_min * factor;

        CuttingConditions {
            cutting_speed_m_min: conditions.cutting_speed_m_min * factor,
            feed_rate_mm_min: new_feed,
            feed_per_tooth_mm: conditions.feed_per_tooth_mm * factor,
            axial_doc_mm: conditions.axial_doc_mm * 0.5, // 절입 깊이 절반
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
        let (k_c11, m_c) = workpiece.kienzle_constants();
        
        let mrr_cm3_min = (conds.axial_doc_mm * conds.radial_doc_mm * conds.feed_rate_mm_min) / 1000.0;
        
        let f_z = conds.feed_per_tooth_mm;
        let cutting_force_n = if f_z > 0.0 {
            conds.axial_doc_mm * f_z.powf(1.0 - m_c) * k_c11
        } else {
            0.0
        };
        
        let spindle_power_kw = (cutting_force_n * conds.cutting_speed_m_min) / (60000.0 * 0.8);

        let l = tool_loc_mm + 10.0;
        let e = 600_000.0;
        let i = (std::f64::consts::PI * tool_diameter_mm.powi(4)) / 64.0;
        let radial_force_n = cutting_force_n * 0.3;
        let tool_deflection_mm = (radial_force_n * l.powi(3)) / (3.0 * e * i);

        PhysicalSimulationResult {
            mrr_cm3_min,
            cutting_force_n,
            spindle_power_kw,
            tool_deflection_mm,
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