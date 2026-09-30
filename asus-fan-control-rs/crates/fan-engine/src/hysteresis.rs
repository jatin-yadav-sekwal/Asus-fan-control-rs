use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct HysteresisConfig {
    /// Smoothing factor for Exponential Moving Average (0.1 = very smooth, 1.0 = instant raw)
    pub ema_alpha: f32,
    /// Delay before stepping down fan speed (prevents rapid rev-down)
    pub ramp_down_cooldown: Duration,
    /// Minimum temperature drop (°C) required before decreasing fan speed
    pub temp_deadband_c: f32,
}

impl Default for HysteresisConfig {
    fn default() -> Self {
        Self {
            ema_alpha: 0.08,
            ramp_down_cooldown: Duration::from_millis(8000),
            temp_deadband_c: 3.0,
        }
    }
}

pub struct HysteresisFilter {
    config: HysteresisConfig,
    smoothed_temp: Option<f32>,
    current_duty_percent: f32,
    last_ramp_down_attempt: Instant,
    peak_temp_before_cool: f32,
}

impl HysteresisFilter {
    pub fn new(config: HysteresisConfig) -> Self {
        Self {
            config,
            smoothed_temp: None,
            current_duty_percent: 0.0,
            last_ramp_down_attempt: Instant::now(),
            peak_temp_before_cool: 0.0,
        }
    }

    /// Updates the Exponential Moving Average with a new raw temperature reading.
    pub fn update_temp(&mut self, raw_temp_c: f32) -> f32 {
        let smoothed = match self.smoothed_temp {
            Some(prev) => prev * (1.0 - self.config.ema_alpha) + raw_temp_c * self.config.ema_alpha,
            None => raw_temp_c,
        };
        self.smoothed_temp = Some(smoothed);
        smoothed
    }

    /// Applies asymmetric ramping and deadband cooldown to the desired fan curve percentage.
    pub fn apply_duty_hysteresis(&mut self, target_percent_from_curve: f32) -> f32 {
        let smoothed = self.smoothed_temp.unwrap_or(0.0);
        if target_percent_from_curve > self.current_duty_percent {
            // Heating up: react immediately for hardware protection
            self.current_duty_percent = target_percent_from_curve;
            self.peak_temp_before_cool = smoothed;
            self.last_ramp_down_attempt = Instant::now();
        } else if target_percent_from_curve < self.current_duty_percent {
            // Cooling down: enforce deadband and cooldown timer to prevent fan pulsating
            let now = Instant::now();
            let elapsed = now.duration_since(self.last_ramp_down_attempt);
            let temp_drop = self.peak_temp_before_cool - smoothed;

            if elapsed >= self.config.ramp_down_cooldown && temp_drop >= self.config.temp_deadband_c {
                // Cooldown satisfied: allow fan speed reduction
                self.current_duty_percent = target_percent_from_curve;
                self.peak_temp_before_cool = smoothed;
                self.last_ramp_down_attempt = now;
            }
        }
        self.current_duty_percent
    }

    /// Feeds a raw temperature measurement, updates EMA, and applies asymmetric ramping.
    /// Returns (smoothed_temp, target_duty_percent).
    pub fn update(&mut self, raw_temp_c: f32, target_percent_from_curve: f32) -> (f32, f32) {
        let smoothed = self.update_temp(raw_temp_c);
        let duty = self.apply_duty_hysteresis(target_percent_from_curve);
        (smoothed, duty)
    }

    pub fn current_duty_percent(&self) -> f32 {
        self.current_duty_percent
    }

    pub fn smoothed_temp(&self) -> Option<f32> {
        self.smoothed_temp
    }

    pub fn reset(&mut self) {
        self.smoothed_temp = None;
        self.current_duty_percent = 0.0;
        self.peak_temp_before_cool = 0.0;
    }
}
