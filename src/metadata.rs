// src/metadata.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PriceTier {
    LowEnd,
    MidRange,
    HighEnd,
}

impl PriceTier {
    pub fn dimensional_tolerance_mm(&self) -> f64 {
        match self {
            Self::LowEnd => 0.01,
            Self::MidRange => 0.005,
            Self::HighEnd => 0.002,
        }
    }

    pub fn predictable_tool_life(&self) -> bool {
        !matches!(self, Self::LowEnd)
    }

    pub fn supports_hard_material(&self) -> bool {
        matches!(self, Self::HighEnd)
    }

    pub fn supports_unmanned_operation(&self) -> bool {
        matches!(self, Self::HighEnd)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Brand {
    pub name: String,
    pub country: Country,
    pub primary_tier: PriceTier,
    pub tier_range: (PriceTier, PriceTier),
    pub signature_coating: Option<String>,
    pub specialties: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Country {
    SouthKorea,
    Japan,
    Germany,
    Sweden,
    Switzerland,
    China,
    Taiwan,
    Other(String),
}

pub fn known_brands() -> Vec<Brand> {
    vec![
        Brand {
            name: "YG-1".into(),
            country: Country::SouthKorea,
            primary_tier: PriceTier::MidRange,
            tier_range: (PriceTier::MidRange, PriceTier::MidRange),
            signature_coating: Some("AlTiN / TiAlN".into()),
            specialties: vec!["자동차 부품".into(), "일반 기계".into(), "금형강".into()],
        },
        Brand {
            name: "OSG".into(),
            country: Country::Japan,
            primary_tier: PriceTier::MidRange,
            tier_range: (PriceTier::MidRange, PriceTier::HighEnd),
            signature_coating: Some("nACo (나노 복합)".into()),
            specialties: vec!["금형".into(), "고경도강".into(), "정밀 부품".into()],
        },
        Brand {
            name: "Sandvik Coromant".into(),
            country: Country::Sweden,
            primary_tier: PriceTier::HighEnd,
            tier_range: (PriceTier::MidRange, PriceTier::HighEnd),
            signature_coating: Some("Invega / Zertis".into()),
            specialties: vec!["항공우주".into(), "난삭재".into(), "무인 양산".into()],
        },
        Brand {
            name: "Fraisa".into(),
            country: Country::Switzerland,
            primary_tier: PriceTier::HighEnd,
            tier_range: (PriceTier::HighEnd, PriceTier::HighEnd),
            signature_coating: Some("X.TEND / 벨로텍".into()),
            specialties: vec!["초정밀".into(), "항공".into(), "의료".into()],
        },
        Brand {
            name: "Seco / Jabro".into(),
            country: Country::Sweden,
            primary_tier: PriceTier::HighEnd,
            tier_range: (PriceTier::MidRange, PriceTier::HighEnd),
            signature_coating: Some("Duratomic / Jabro".into()),
            specialties: vec!["인코넬".into(), "티타늄".into(), "금형".into()],
        },
        Brand {
            name: "Mitsubishi Materials".into(),
            country: Country::Japan,
            primary_tier: PriceTier::MidRange,
            tier_range: (PriceTier::MidRange, PriceTier::HighEnd),
            signature_coating: Some("Al-rich / Miracle".into()),
            specialties: vec!["자동차".into(), "금형".into(), "일반 기계".into()],
        },
        Brand {
            name: "Korloy".into(),
            country: Country::SouthKorea,
            primary_tier: PriceTier::MidRange,
            tier_range: (PriceTier::MidRange, PriceTier::MidRange),
            signature_coating: Some("PC / PCXN".into()),
            specialties: vec!["자동차".into(), "일반 기계".into()],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_price_tier_ordering() {
        assert!(PriceTier::LowEnd < PriceTier::MidRange);
        assert!(PriceTier::MidRange < PriceTier::HighEnd);
    }

    #[test]
    fn test_high_end_capabilities() {
        assert!(PriceTier::HighEnd.predictable_tool_life());
        assert!(PriceTier::HighEnd.supports_hard_material());
        assert!(!PriceTier::LowEnd.supports_unmanned_operation());
    }
}