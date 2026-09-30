use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::RwLock;
use tokio::sync::broadcast;
use tracing::{info, warn};

use asus_driver::SafeAsusDriver;
use crate::hysteresis::{HysteresisConfig, HysteresisFilter};
use crate::profile::FanProfile;
use crate::watchdog::ThermalWatchdog;

/// How often the target duty is re-asserted to the EC even when unchanged.
/// Covers the EC silently dropping back to the BIOS curve (e.g. after a
/// resume), without spamming a write every single tick.
const WRITE_HEARTBEAT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub struct EngineTelemetrySnapshot {
    pub raw_cpu_temp: u32,
    pub smoothed_cpu_temp: f32,
    pub target_fan_percent: u8,
    /// Duty the hardware acknowledged, `None` if the last write failed.
    pub applied_fan_percent: Option<u8>,
    pub fan_rpms: Vec<u32>,
    pub is_emergency_active: bool,
    pub active_profile_name: String,
    /// Most recent read/write failure, surfaced in the UI instead of hiding.
    pub last_error: Option<String>,
}

impl EngineTelemetrySnapshot {
    pub fn placeholder() -> Self {
        Self {
            raw_cpu_temp: 0,
            smoothed_cpu_temp: 0.0,
            target_fan_percent: 0,
            applied_fan_percent: None,
            fan_rpms: Vec::new(),
            is_emergency_active: false,
            active_profile_name: "—".to_string(),
            last_error: None,
        }
    }
}

pub struct ThermalEngine {
    driver: SafeAsusDriver,
    active_profile: Arc<RwLock<FanProfile>>,
    hysteresis: Arc<RwLock<HysteresisFilter>>,
    watchdog: Arc<RwLock<ThermalWatchdog>>,
    telemetry_tx: broadcast::Sender<EngineTelemetrySnapshot>,
    is_running: Arc<std::sync::atomic::AtomicBool>,
    worker: parking_lot::Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl ThermalEngine {
    pub fn new(driver: SafeAsusDriver) -> Self {
        let (telemetry_tx, _) = broadcast::channel(32);
        Self {
            driver,
            active_profile: Arc::new(RwLock::new(FanProfile::Balanced)),
            hysteresis: Arc::new(RwLock::new(HysteresisFilter::new(HysteresisConfig::default()))),
            watchdog: Arc::new(RwLock::new(ThermalWatchdog::default())),
            telemetry_tx,
            is_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            worker: parking_lot::Mutex::new(None),
        }
    }

    pub fn subscribe_telemetry(&self) -> broadcast::Receiver<EngineTelemetrySnapshot> {
        self.telemetry_tx.subscribe()
    }

    pub fn set_profile(&self, profile: FanProfile) {
        info!("Switching thermal profile to: {}", profile.name());
        *self.active_profile.write() = profile;
        self.hysteresis.write().reset();
    }

    pub fn active_profile(&self) -> FanProfile {
        self.active_profile.read().clone()
    }

    pub fn start(&self, poll_interval: Duration) {
        self.is_running.store(true, std::sync::atomic::Ordering::SeqCst);
        let driver = self.driver.clone();
        let profile_lock = self.active_profile.clone();
        let hysteresis_lock = self.hysteresis.clone();
        let watchdog_lock = self.watchdog.clone();
        let telemetry_tx = self.telemetry_tx.clone();
        let is_running = self.is_running.clone();

        let handle = std::thread::Builder::new()
            .name("thermal-engine-worker".into())
            .spawn(move || {
                info!("Thermal automation loop started (Interval: {:?})", poll_interval);

                let mut last_written: Option<u8> = None;
                let mut last_write_at = Instant::now() - WRITE_HEARTBEAT;
                let mut last_rpms: Vec<u32> = Vec::new();
                let mut last_error: Option<String> = None;
                let mut tick: u64 = 0;

                while is_running.load(std::sync::atomic::Ordering::SeqCst) {
                    // 1. Read CPU temperature only — RPM is read *after* the duty
                    //    write so the displayed value reflects the new target.
                    let raw_temp = {
                        let mut driver_guard = driver.lock();
                        match driver_guard.get_cpu_temperature() {
                            Ok(t) => t,
                            Err(e) => {
                                last_error = Some(e.to_string());
                                0
                            }
                        }
                    };

                    // 2. Evaluate watchdog safety limits
                    let emergency_duty = watchdog_lock.write().check(raw_temp as f32);
                    let is_emergency = emergency_duty.is_some();

                    let active_prof = profile_lock.read().clone();
                    let active_name = active_prof.name().to_string();

                    // Always update EMA filter once per tick so telemetry shows properly smoothed temperature
                    let smoothed_temp = hysteresis_lock.write().update_temp(raw_temp as f32);

                    let target_percent = if let Some(duty) = emergency_duty {
                        // Critical overheat: force 100% duty immediately
                        duty as u8
                    } else {
                        match active_prof {
                            FanProfile::Manual(percent) => percent,
                            // BIOS default: release the EC (target 0 == BIOS control)
                            FanProfile::BiosDefault => 0,
                            _ => {
                                if let Some(curve) = active_prof.to_curve() {
                                    let curve_target = curve.evaluate(smoothed_temp);
                                    let final_duty =
                                        hysteresis_lock.write().apply_duty_hysteresis(curve_target);
                                    final_duty.round() as u8
                                } else {
                                    0
                                }
                            }
                        }
                    };

                    // 3. Write the target duty to hardware only when it changed (plus a
                    //    slow heartbeat). Rewriting every tick churned the EC inside the
                    //    tach read window and made RPM look frozen.
                    let applied = {
                        let needs_write = last_written != Some(target_percent)
                            || last_write_at.elapsed() >= WRITE_HEARTBEAT;
                        if !needs_write {
                            last_written
                        } else {
                            let mut driver_guard = driver.lock();
                            let result = if target_percent == 0 {
                                driver_guard.reset_all_to_bios()
                            } else {
                                driver_guard.set_all_fans_percent(target_percent)
                            };
                            match result {
                                Ok(()) => {
                                    last_written = Some(target_percent);
                                    last_write_at = Instant::now();
                                    last_error = None;
                                    Some(target_percent)
                                }
                                Err(e) => {
                                    warn!("Failed to apply {}% duty: {}", target_percent, e);
                                    last_error = Some(e.to_string());
                                    last_written = None;
                                    None
                                }
                            }
                        }
                    };

                    // 4. Read RPM after the write so the number is current, not stale
                    let fan_rpms = {
                        let driver_guard = driver.lock();
                        match driver_guard.get_all_fan_rpms() {
                            Ok(rpms) => {
                                if !rpms.is_empty() {
                                    last_rpms = rpms.clone();
                                }
                                rpms
                            }
                            Err(e) => {
                                warn!("Fan RPM read failed: {}", e);
                                last_error = Some(e.to_string());
                                last_rpms.clone()
                            }
                        }
                    };

                    // 5. Broadcast live snapshot
                    let snapshot = EngineTelemetrySnapshot {
                        raw_cpu_temp: raw_temp,
                        smoothed_cpu_temp: smoothed_temp,
                        target_fan_percent: target_percent,
                        applied_fan_percent: applied,
                        fan_rpms,
                        is_emergency_active: is_emergency,
                        active_profile_name: active_name,
                        last_error: last_error.clone(),
                    };

                    tick += 1;
                    if tick % 5 == 0 {
                        info!(
                            "telemetry: {:.1}C -> {}% (hw {}%) rpm={:?} err={}",
                            smoothed_temp,
                            target_percent,
                            applied.map(|v| v.to_string()).unwrap_or_else(|| "-".into()),
                            snapshot.fan_rpms,
                            snapshot.last_error.as_deref().unwrap_or("none"),
                        );
                    }

                    let _ = telemetry_tx.send(snapshot);

                    std::thread::sleep(poll_interval);
                }

                info!("Thermal automation loop stopped.");
            })
            .expect("Failed to spawn thermal thread");
        *self.worker.lock() = Some(handle);
    }

    pub fn stop(&self) {
        self.is_running.store(false, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.worker.lock().take() {
            let _ = handle.join();
        }
        info!("Thermal engine stopped; returning fans to BIOS control...");
        match self.driver.lock().reset_all_to_bios() {
            Ok(()) => info!("Fans restored to BIOS control on shutdown."),
            Err(e) => warn!("Could not restore BIOS control on shutdown: {}", e),
        }
    }
}
