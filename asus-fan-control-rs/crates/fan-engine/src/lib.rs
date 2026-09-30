pub mod curve;
pub mod engine;
pub mod hysteresis;
pub mod profile;
pub mod watchdog;

pub use curve::{CurvePoint, FanCurve};
pub use engine::{EngineTelemetrySnapshot, ThermalEngine};
pub use hysteresis::{HysteresisConfig, HysteresisFilter};
pub use profile::FanProfile;
pub use watchdog::ThermalWatchdog;
