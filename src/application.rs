use serde::{Deserialize, Serialize};
use crate::metadata::PriceTier;
use crate::cutting::WorkpieceMaterial;

/// 가공 목적 분류
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MachiningPurpose {
    /// 황삭 (Roughing) - 형상 크게 파내기
    Roughing,
    /// 정삭 (Finishing) - 치수/조도 맞추기
    Finishing,
    /// 반정삭 (Semi-finishing)
    SemiFinishing,
    /// 홈 가공 (Slotting)
    Slotting,
    /// 포켓 가공
    Pocketing,
    /// 프로파일링 (외곽)
    Profiling,
}

/// 적용 산업 분야
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Industry {
    /// 항공·우주
    Aerospace,
    /// 의료 (인공관절, 임플란트)
    Medical,
    /// 자동차
    Automotive,
    /// 금형
    MoldDie,
    /// 일반 기계
    GeneralMachining,
    /// 전자/반도체
    Electronics,
    /// 취미/시제품
    HobbyPrototype,
}

impl Industry {
    /// 해당 산업에서 요구되는 최소 가격 등급
    pub fn minimum_tier(&self) -> PriceTier {
        match self {
            Self::Aerospace | Self::Medical => PriceTier::HighEnd,
            Self::Automotive | Self::MoldDie => PriceTier::MidRange,
            Self::GeneralMachining => PriceTier::MidRange,
            Self::Electronics => PriceTier::MidRange,
            Self::HobbyPrototype => PriceTier::LowEnd,
        }
    }

    /// 해당 산업의 주요 피삭재
    pub fn typical_materials(&self) -> Vec<WorkpieceMaterial> {
        match self {
            Self::Aerospace => vec![
                WorkpieceMaterial::Titanium,
                WorkpieceMaterial::Inconel,
                WorkpieceMaterial::SuperAlloy,
            ],
            Self::Medical => vec![
                WorkpieceMaterial::Titanium,
                WorkpieceMaterial::StainlessSteel,
            ],
            Self::Automotive => vec![
                WorkpieceMaterial::AlloySteel { hardness_hrc: 45 },
                WorkpieceMaterial::Aluminum,
                WorkpieceMaterial::CarbonSteel,
            ],
            Self::MoldDie => vec![
                WorkpieceMaterial::AlloySteel { hardness_hrc: 55 },
                WorkpieceMaterial::AlloySteel { hardness_hrc: 60 },
            ],
            Self::GeneralMachining => vec![
                WorkpieceMaterial::CarbonSteel,
                WorkpieceMaterial::Aluminum,
                WorkpieceMaterial::StainlessSteel,
            ],
            Self::Electronics => vec![
                WorkpieceMaterial::Aluminum,
                WorkpieceMaterial::CFRP,
            ],
            Self::HobbyPrototype => vec![
                WorkpieceMaterial::Aluminum,
                WorkpieceMaterial::CarbonSteel,
            ],
        }
    }
}

/// 용도-공구 매칭 정보
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationProfile {
    pub industry: Industry,
    pub purpose: MachiningPurpose,
    pub recommended_tier: PriceTier,
    pub workpiece: WorkpieceMaterial,
    pub note: String,
}

/// 가격군별 용도 매칭 테이블 생성
pub fn build_application_table() -> Vec<ApplicationProfile> {
    vec![
        ApplicationProfile {
            industry: Industry::HobbyPrototype,
            purpose: MachiningPurpose::Roughing,
            recommended_tier: PriceTier::LowEnd,
            workpiece: WorkpieceMaterial::Aluminum,
            note: "목업/시가공, 치수 정밀도 불필요".into(),
        },
        ApplicationProfile {
            industry: Industry::Automotive,
            purpose: MachiningPurpose::Roughing,
            recommended_tier: PriceTier::MidRange,
            workpiece: WorkpieceMaterial::CarbonSteel,
            note: "자동차 부품 양산, 수명 예측 필수".into(),
        },
        ApplicationProfile {
            industry: Industry::MoldDie,
            purpose: MachiningPurpose::Finishing,
            recommended_tier: PriceTier::HighEnd,
            workpiece: WorkpieceMaterial::AlloySteel { hardness_hrc: 60 },
            note: "HRC 60 이상 고경도 금형 정삭, 무인 가공".into(),
        },
        ApplicationProfile {
            industry: Industry::Aerospace,
            purpose: MachiningPurpose::Roughing,
            recommended_tier: PriceTier::HighEnd,
            workpiece: WorkpieceMaterial::Inconel,
            note: "항공 엔진 부품, 1000°C 이상 내열 코팅 필수".into(),
        },
        ApplicationProfile {
            industry: Industry::Medical,
            purpose: MachiningPurpose::Finishing,
            recommended_tier: PriceTier::HighEnd,
            workpiece: WorkpieceMaterial::Titanium,
            note: "인공관절, ±0.002mm 공차, 전수 검사".into(),
        },
    ]
}