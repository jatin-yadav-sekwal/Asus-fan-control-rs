use serde::{Deserialize, Serialize};
use crate::curve::FanCurve;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FanProfile {
    BiosDefault,
    Silent,
    Balanced,
    Turbo,
    FullSpeed,
    Manual(u8),
    Custom(FanCurve),
}

impl FanProfile {
    pub fn name(&self) -> String {
        match self {
            Self::BiosDefault => "BIOS Default".to_string(),
            Self::Silent => "Silent".to_string(),
            Self::Balanced => "Balanced".to_string(),
            Self::Turbo => "Turbo".to_string(),
            Self::FullSpeed => "Full Speed".to_string(),
            Self::Manual(p) => format!("Manual ({}%)", p),
            Self::Custom(c) => c.name.clone(),
        }
    }

    pub fn to_curve(&self) -> Option<FanCurve> {
        match self {
            Self::BiosDefault => None,
            Self::Silent => Some(FanCurve::silent()),
            Self::Balanced => Some(FanCurve::balanced()),
            Self::Turbo => Some(FanCurve::turbo()),
            Self::FullSpeed => Some(FanCurve::full_speed()),
            Self::Manual(p) => Some(FanCurve::constant(*p)),
            Self::Custom(curve) => Some(curve.clone()),
        }
    }
}
