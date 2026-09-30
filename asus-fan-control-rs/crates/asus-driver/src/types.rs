#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FanId(pub u8);

impl std::fmt::Display for FanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Fan #{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FanPwm(pub u8);

impl FanPwm {
    pub const MIN: Self = Self(0);
    pub const MAX: Self = Self(255);

    pub fn from_percent(percent: u8) -> Self {
        let clamped = percent.min(100);
        let raw = ((clamped as f32 / 100.0) * 255.0).round() as u8;
        Self(raw)
    }

    pub fn to_percent(&self) -> u8 {
        ((self.0 as f32 / 255.0) * 100.0).round() as u8
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanTelemetry {
    pub id: FanId,
    pub rpm: u32,
    pub target_percent: Option<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SystemTelemetry {
    pub cpu_temp_c: u32,
    pub fans: Vec<FanTelemetry>,
}
