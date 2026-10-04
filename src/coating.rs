use serde::{Deserialize, Serialize};

/// 코팅 증착 방식
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DepositionMethod {
    /// 물리적 기상 증착 (Physical Vapor Deposition)
    PVD,
    /// 화학적 기상 증착 (Chemical Vapor Deposition)
    CVD,
    /// 고전압 펄스 마그네트론 스퍼터링
    /// - 초고전압 플라즈마로 이온 완전 분쇄
    /// - 표면이 유리처럼 매끄럽고 촘촘
    HiPIMS,
    /// 코팅 없음 (Bare)
    None,
}

/// 코팅 층 단일 레이어
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoatingLayer {
    /// 층 순서 (1 = 베이스 쪽)
    pub layer_index: u8,
    /// 코팅 물질
    pub compound: CoatingCompound,
    /// 층 두께 (μm)
    pub thickness_um: f64,
    /// 층 역할
    pub role: LayerRole,
}

/// 코팅 화합물 종류
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CoatingCompound {
    /// TiAlN - 범용 (내열 ~800°C)
    TiAlN,
    /// AlTiN - 고경도강 (~900°C)
    AlTiN,
    /// AlCrN - 고열 안정성 (~1100°C)
    AlCrN,
    /// TiSiN - 나노 결정질, 초고경도
    TiSiN,
    /// CrN - 내마모, 인성 우수
    CrN,
    /// TiN - 범용, 저마찰
    TiN,
    /// DLC (Diamond-Like Carbon)
    DLC,
    /// nACo - 나노 복합 (Osg 등)
    NACo,
    /// 독자 배합 (브랜드 특허)
    Proprietary { name: String },
}

/// 코팅 층의 역할
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayerRole {
    /// 베이스 밀착력 확보 (Adhesion)
    Adhesion,
    /// 충격 흡수 (Toughness)
    Toughness,
    /// 내열 / 내산화 (Thermal barrier)
    ThermalBarrier,
    /// 내마모 / 경도 (Wear resistance)
    WearResistance,
    /// 구배층 (Gradient - 열팽창 계수 매칭)
    GradientTransition,
}

/// 완전한 코팅 스펙
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoatingSpec {
    /// 코팅 이름 (예: "AlTiN HiPIMS Multi-layer")
    pub name: String,
    /// 증착 방식
    pub method: DepositionMethod,
    /// 층 구성 (다층)
    pub layers: Vec<CoatingLayer>,
    /// 총 두께 (μm)
    pub total_thickness_um: f64,
    /// 표면 조도 Ra (μm) - HiPIMS는 극도로 매끄러움
    pub surface_roughness_ra: f64,
    /// 최대 내열 온도 (°C)
    pub max_temp_c: u32,
    /// 표면 경도 (HV)
    pub hardness_hv: u32,
    /// 잔류 응력 제어 여부
    pub residual_stress_controlled: bool,
    /// 전처리 (Edge Honing) 적용 여부
    pub edge_honing_applied: bool,
}

impl CoatingSpec {
    /// 코팅이 난삭재(인코넬, 티타늄 등)용인지 판별
    pub fn is_for_hard_material(&self) -> bool {
        self.max_temp_c >= 1000
            && self.layers.iter().any(|l| {
                matches!(
                    l.compound,
                    CoatingCompound::AlCrN
                        | CoatingCompound::TiSiN
                        | CoatingCompound::NACo
                        | CoatingCompound::Proprietary { .. }
                )
            })
    }

    /// HiPIMS 등급 여부 (표면 조도 기준)
    pub fn is_hipims_grade(&self) -> bool {
        matches!(self.method, DepositionMethod::HiPIMS)
            && self.surface_roughness_ra < 0.05
    }

    /// 다층 코팅의 크랙 전파 방지 여부
    pub fn has_crack_propagation_barrier(&self) -> bool {
        self.layers.len() >= 3
            && self.layers.iter().any(|l| matches!(l.role, LayerRole::Toughness))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hipims_coating() {
        let coating = CoatingSpec {
            name: "HiPIMS AlTiN Multi".into(),
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
                    compound: CoatingCompound::AlTiN,
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
            max_temp_c: 1100,
            hardness_hv: 3800,
            residual_stress_controlled: true,
            edge_honing_applied: true,
        };

        assert!(coating.is_hipims_grade());
        assert!(coating.is_for_hard_material());
        assert!(coating.has_crack_propagation_barrier());
    }
}