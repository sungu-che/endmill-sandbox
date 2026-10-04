use serde::{Deserialize, Serialize};
use crate::metadata::{Brand, PriceTier};
use crate::coating::CoatingSpec;
use crate::cutting::{CuttingConditions, WorkpieceMaterial};
use crate::material::ToolBaseMaterial;

/// 공구 형상 (날 형태)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FluteGeometry {
    /// 2날 - 알루미늄/비철, 칩 배출 우수
    TwoFlute,
    /// 3날 - 범용
    ThreeFlute,
    /// 4날 - 범용/강 가공
    FourFlute,
    /// 5날 - 고이송
    FiveFlute,
    /// 6날 이상 - 고경도강 정삭
    MultiFlute { count: u8 },
    /// 볼노즈 (Ball Nose) - 3D 형상
    BallNose,
    /// 라운드노즈 (Corner Radius)
    CornerRadius { radius_mm: f64 },
}

/// 공구 전체 사양 (End Mill Specification)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndMillSpec {
    /// 모델명 / 품번
    pub model: String,
    /// 제조사
    pub brand: Brand,
    /// 가격 등급
    pub tier: PriceTier,

    // === 형상 ===
    /// 공구 직경 (mm)
    pub diameter_mm: f64,
    /// 유효 절삭 길이 (mm) - LOC (Length of Cut)
    pub length_of_cut_mm: f64,
    /// 전체 길이 (mm) - OAL (Overall Length)
    pub overall_length_mm: f64,
    /// 샹크 직경 (mm)
    pub shank_diameter_mm: f64,
    /// 날 수 / 형상
    pub flute_geometry: FluteGeometry,
    /// 헬릭스 각도 (°)
    pub helix_angle_deg: f64,
    /// 부등 분할 여부 (채터링 억제용)
    pub variable_pitch: bool,

    // === 소재 & 코팅 ===
    /// 베이스 소재
    pub base_material: ToolBaseMaterial,
    /// 코팅 사양
    pub coating: Option<CoatingSpec>,

    // === 적용 ===
    /// 권장 피삭재 목록
    pub recommended_workpieces: Vec<WorkpieceMaterial>,
    /// 권장 절삭 조건 (참고값)
    pub reference_conditions: Option<CuttingConditions>,

    // === 메타 ===
    /// 설명/비고
    pub description: String,
}

impl EndMillSpec {
    /// 난삭재 가공 가능 여부 종합 판정
    pub fn can_machine_hard_materials(&self) -> bool {
        self.base_material.can_machine_hard_material()
            && self
                .coating
                .as_ref()
                .map(|c| c.is_for_hard_material())
                .unwrap_or(false)
    }

    /// 무인 야간 가공 적합 여부
    pub fn suitable_for_unmanned_op(&self) -> bool {
        self.tier.predictable_tool_life()
            && self.tier.supports_unmanned_operation()
            && self.variable_pitch // 채터링 억제 설계
    }

    /// 저가 공구 대비 예상 수명 배수
    pub fn estimated_life_multiplier_vs_low_end(&self) -> f64 {
        match self.tier {
            PriceTier::LowEnd => 1.0,
            PriceTier::MidRange => 3.0,
            PriceTier::HighEnd => 8.0, // 코팅 + 정밀도 + 부등분할
        }
    }
}

/// 샘플 공구 생성 (테스트/데모용)
pub fn sample_high_end_endmill() -> EndMillSpec {
    use crate::coating::*;
    use crate::material::*;

    EndMillSpec {
        model: "OSG A-Brand ADO-SUS-4D".into(),
        brand: crate::metadata::known_brands()
            .into_iter()
            .find(|b| b.name == "OSG")
            .unwrap(),
        tier: PriceTier::HighEnd,
        diameter_mm: 10.0,
        length_of_cut_mm: 25.0,
        overall_length_mm: 75.0,
        shank_diameter_mm: 10.0,
        flute_geometry: FluteGeometry::FourFlute,
        helix_angle_deg: 40.0,
        variable_pitch: true,
        base_material: ToolBaseMaterial::SolidCarbide(CarbideGrade::SubMicron {
            grain_size_um: 0.4,
            cobalt_percent: 10.0,
        }),
        coating: Some(CoatingSpec {
            name: "OSG nACo HiPIMS".into(),
            method: DepositionMethod::HiPIMS,
            layers: vec![
                CoatingLayer {
                    layer_index: 1,
                    compound: CoatingCompound::CrN,
                    thickness_um: 0.5,
                    role: LayerRole::Adhesion,
                },
                CoatingLayer {
                    layer_index: 2,
                    compound: CoatingCompound::Proprietary {
                        name: "nACo Tough".into(),
                    },
                    thickness_um: 1.5,
                    role: LayerRole::Toughness,
                },
                CoatingLayer {
                    layer_index: 3,
                    compound: CoatingCompound::TiSiN,
                    thickness_um: 1.0,
                    role: LayerRole::WearResistance,
                },
            ],
            total_thickness_um: 3.0,
            surface_roughness_ra: 0.03,
            max_temp_c: 1200,
            hardness_hv: 4200,
            residual_stress_controlled: true,
            edge_honing_applied: true,
        }),
        recommended_workpieces: vec![
            WorkpieceMaterial::StainlessSteel,
            WorkpieceMaterial::Titanium,
            WorkpieceMaterial::AlloySteel { hardness_hrc: 55 },
        ],
        reference_conditions: Some(CuttingConditions {
            cutting_speed_m_min: 60.0,
            feed_rate_mm_min: 900.0,
            feed_per_tooth_mm: 0.05,
            axial_doc_mm: 10.0,
            radial_doc_mm: 3.0,
            spindle_rpm: 1910,
            coolant: crate::cutting::CoolantType::ThroughTool,
        }),
        description: "OSG A-Brand 스테인리스/티타늄 전용 4날, HiPIMS 나노 코팅".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sample_high_end() {
        let spec = sample_high_end_endmill();
        assert!(spec.can_machine_hard_materials());
        assert!(spec.suitable_for_unmanned_op());
        assert_eq!(spec.estimated_life_multiplier_vs_low_end(), 8.0);
    }

    #[test]
    fn test_json_roundtrip() {
        let spec = sample_high_end_endmill();
        let json = serde_json::to_string_pretty(&spec).unwrap();
        let deserialized: EndMillSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.model, spec.model);
    }
}