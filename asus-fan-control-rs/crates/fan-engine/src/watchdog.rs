use tracing::{error, info};

pub struct ThermalWatchdog {
    pub critical_temp_c: f32,
    pub recovery_temp_c: f32,
    is_emergency_active: bool,
}

impl Default for ThermalWatchdog {
    fn default() -> Self {
        Self {
            critical_temp_c: 92.0,
            recovery_temp_c: 84.0,
            is_emergency_active: false,
        }
    }
}

impl ThermalWatchdog {
    pub fn new(critical_temp_c: f32, recovery_temp_c: f32) -> Self {
        Self {
            critical_temp_c,
            recovery_temp_c,
            is_emergency_active: false,
        }
    }

    /// Evaluates if emergency thermal intervention is required.
    /// Returns Some(100.0) if thermal emergency is active (overheating), or None if normal curve applies.
    pub fn check(&mut self, current_temp_c: f32) -> Option<f32> {
        if current_temp_c >= self.critical_temp_c {
            if !self.is_emergency_active {
                error!(
                    "THERMAL EMERGENCY: Current temperature ({:.1}°C) exceeds critical safety limit ({:.1}°C)! Forcing 100% fan duty.",
                    current_temp_c, self.critical_temp_c
                );
                self.is_emergency_active = true;
            }
            Some(100.0)
        } else if self.is_emergency_active {
            if current_temp_c <= self.recovery_temp_c {
                info!(
                    "Thermal recovery: Temperature lowered to {:.1}°C. Reverting to normal fan curve.",
                    current_temp_c
                );
                self.is_emergency_active = false;
                None
            } else {
                // Keep emergency 100% active until cooled below recovery threshold
                Some(100.0)
            }
        } else {
            None
        }
    }

    pub fn is_emergency_active(&self) -> bool {
        self.is_emergency_active
    }
}
