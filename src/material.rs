use serde::{Deserialize, Serialize};

/// 공구 베이스 소재 분류
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToolBaseMaterial {
    /// 고속도강 (High Speed Steel)
    /// - 알루미늄, 일반 강 가공
    /// - 가성비 용도
    HSS(HSSGrade),

    /// 초경합금 (Solid Carbide / Tungsten Carbide)
    /// - 현재 시장의 절대 주력
    /// - 텅스텐 카바이드 + 코발트 바인더
    SolidCarbide(CarbideGrade),

    /// CBN (Cubic Boron Nitride)
    /// - 초고경도강 전용
    CBN,

    /// PCD (Polycrystalline Diamond)
    /// - 알루미늄/비철 전용
    PCD,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HSSGrade {
    /// M2 - 범용
    M2,
    /// M35 - 코발트 첨가 (5%)
    M35,
    /// M42 - 코발트 첨가 (8%)
    M42,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CarbideGrade {
    /// 미세 입자 (Fine Grain) - 일반 가공
    FineGrain {
        /// WC 입자 크기 (μm)
        grain_size_um: f64,
        /// 코발트 바인더 함량 (%)
        cobalt_percent: f64,
    },
    /// 초미세 입자 (Sub-micron) - 고경도강
    SubMicron {
        grain_size_um: f64,
        cobalt_percent: f64,
    },
    /// 초초미세 입자 (Ultra-fine) - 초정밀
    UltraFine {
        grain_size_um: f64,
        cobalt_percent: f64,
    },
}

impl ToolBaseMaterial {
    /// 소재의 경도 (HRC 또는 HV)
    pub fn hardness_hra(&self) -> f64 {
        match self {
            Self::HSS(HSSGrade::M2) => 64.0,
            Self::HSS(HSSGrade::M35) => 66.0,
            Self::HSS(HSSGrade::M42) => 68.0,
            Self::SolidCarbide(CarbideGrade::FineGrain { .. }) => 90.0,
            Self::SolidCarbide(CarbideGrade::SubMicron { .. }) => 92.0,
            Self::SolidCarbide(CarbideGrade::UltraFine { .. }) => 93.5,
            Self::CBN => 96.0,
            Self::PCD => 98.0,
        }
    }

    /// 최대 사용 온도 (°C)
    pub fn max_operating_temp_c(&self) -> u32 {
        match self {
            Self::HSS(_) => 600,
            Self::SolidCarbide(_) => 1000,
            Self::CBN => 1400,
            Self::PCD => 700, // 다이아몬드는 고온에서 흑연화
        }
    }

    /// 난삭재 가공 가능 여부
    pub fn can_machine_hard_material(&self) -> bool {
        matches!(
            self,
            Self::SolidCarbide(CarbideGrade::SubMicron { .. })
                | Self::SolidCarbide(CarbideGrade::UltraFine { .. })
                | Self::CBN
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_carbide_hardness() {
        let fine = ToolBaseMaterial::SolidCarbide(CarbideGrade::FineGrain {
            grain_size_um: 0.8,
            cobalt_percent: 10.0,
        });
        assert!(fine.hardness_hra() > 89.0);
        assert!(fine.max_operating_temp_c() >= 1000);
    }
}