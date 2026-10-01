use std::sync::Arc;
use parking_lot::Mutex;
use tracing::{debug, info, warn};

use crate::acpi::AcpiDevice;
use crate::error::DriverError;
use crate::ffi::RawAsusWinIO;
use crate::ipc::PipeClient;
use crate::types::{FanId, FanPwm, FanTelemetry, SystemTelemetry};

pub enum DriverBackend {
    Acpi(AcpiDevice),
    WinIO(RawAsusWinIO),
    /// Forwards every call over the control pipe to the SYSTEM helper. Used by
    /// the GUI, which cannot open `\\.\AsusSAIO` itself.
    Remote(PipeClient),
}

pub struct AsusDriver {
    backend: DriverBackend,
    fan_count: u8,
    thermal_reader: Option<crate::thermal::WindowsThermalReader>,
    last_known_temp: u32,
}

impl AsusDriver {
    pub fn new() -> Result<Self, DriverError> {
        // 1. Primary: AsusWinIO64.dll (exact same backend used by the working AsusFanControl app)
        match RawAsusWinIO::load() {
            Ok(raw) => {
                info!("Initializing primary AsusWinIO driver backend (matching AsusFanControl)...");
                crate::service::preflight_repair();

                // \\.\AsusSAIO only answers to NT AUTHORITY\SYSTEM. Anything else makes
                // every HealthyTable_* export return -1, which must be a hard error and
                // never a silent "0 RPM".
                let mut raw_count;
                let mut rpm_probe = -1i32;
                let mut attempt = 0u8;
                loop {
                    unsafe {
                        (raw.initialize_win_io)();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(150));

                    raw_count = unsafe { (raw.healthy_table_fan_counts)() };
                    if raw_count > 0 {
                        rpm_probe = unsafe {
                            (raw.healthy_table_set_fan_index)(0);
                            std::thread::sleep(std::time::Duration::from_millis(25));
                            (raw.healthy_table_fan_rpm)()
                        };
                        if rpm_probe >= 0 {
                            break;
                        }
                    }

                    attempt += 1;
                    if attempt >= 3 {
                        break;
                    }
                    warn!(
                        "AsusWinIO probe attempt {} failed (fan_count={}, rpm={}); re-initializing",
                        attempt, raw_count, rpm_probe
                    );
                    unsafe {
                        (raw.shutdown_win_io)();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }

                if raw_count <= 0 || rpm_probe < 0 {
                    warn!(
                        "AsusWinIO backend unresponsive: fan_count={}, rpm_probe={}",
                        raw_count, rpm_probe
                    );
                    if !crate::service::is_local_system() {
                        return Err(DriverError::ElevationRequired);
                    }
                    return Err(DriverError::HealthyTableUnresponsive {
                        fan_count: raw_count,
                        rpm: rpm_probe,
                    });
                }

                let fan_count = (raw_count as u8).clamp(1, 16);
                info!(
                    "AsusWinIO healthy: fan_count={}, fan0 idle rpm={}",
                    fan_count, rpm_probe
                );

                let mut thermal_reader = crate::thermal::WindowsThermalReader::new();
                let raw_temp = unsafe { (raw.thermal_read_cpu_temperature)() as u32 };
                let last_known_temp = if (15..=115).contains(&raw_temp) {
                    raw_temp
                } else if let Some(ref mut tr) = thermal_reader {
                    tr.read_temperature_c().unwrap_or(55)
                } else {
                    55
                };

                info!(
                    "AsusDriver (WinIO backend) active. Fan count: {}. Active CPU Temp: {}°C",
                    fan_count, last_known_temp
                );

                return Ok(Self {
                    backend: DriverBackend::WinIO(raw),
                    fan_count,
                    thermal_reader,
                    last_known_temp,
                });
            }
            Err(e) => {
                warn!("AsusWinIO64.dll unavailable ({}); falling back to ATKACPI...", e);
            }
        }

        // 2. Fallback: direct ASUS ACPI device (\\.\ATKACPI)
        match AcpiDevice::open() {
            Ok(acpi) => {
                info!("Fallback ATKACPI hardware backend initialized.");
                let cpu_rpm = acpi.get_fan_rpm(0).ok().flatten();
                let gpu_rpm = acpi.get_fan_rpm(1).ok().flatten();
                info!("Initial ACPI Fan RPM probe: CPU = {:?}, GPU = {:?}", cpu_rpm, gpu_rpm);

                let fan_count = 2u8;
                let mut thermal_reader = crate::thermal::WindowsThermalReader::new();

                let acpi_temp = acpi.get_cpu_temperature().ok().flatten();
                let last_known_temp = if let Some(t) = acpi_temp {
                    t
                } else if let Some(ref mut tr) = thermal_reader {
                    tr.read_temperature_c().unwrap_or(55)
                } else {
                    55
                };

                info!(
                    "AsusDriver (ACPI backend) active. Fan count: {}. Active CPU Temp: {}°C",
                    fan_count, last_known_temp
                );

                return Ok(Self {
                    backend: DriverBackend::Acpi(acpi),
                    fan_count,
                    thermal_reader,
                    last_known_temp,
                });
            }
            Err(e) => {
                warn!("ATKACPI device unavailable ({})", e);
            }
        }

        Err(DriverError::InitializationFailed)
    }

    pub fn fan_count(&self) -> u8 {
        self.fan_count
    }

    /// Attaches to the SYSTEM helper instead of loading the driver locally.
    ///
    /// This is the only constructor the GUI may use: an elevated administrator
    /// is not enough, `\\.\AsusSAIO` answers to `NT AUTHORITY\SYSTEM` alone.
    pub fn connect(pipe_name: &str) -> Result<Self, DriverError> {
        let client = PipeClient::connect(pipe_name)?;
        let (fan_count, initial_temp) = client.hello()?;
        info!(
            "Driver attached over {}: fan_count={}, cpu={}°C",
            pipe_name, fan_count, initial_temp
        );
        Ok(Self {
            backend: DriverBackend::Remote(client),
            fan_count,
            thermal_reader: None,
            last_known_temp: if (15..=115).contains(&initial_temp) {
                initial_temp
            } else {
                55
            },
        })
    }

    pub fn get_fan_rpm(&self, fan_idx: u8) -> Result<u32, DriverError> {
        self.validate_fan_index(fan_idx)?;
        match &self.backend {
            DriverBackend::Acpi(acpi) => {
                let rpm = acpi.get_fan_rpm(fan_idx)?.unwrap_or(0);
                Ok(rpm)
            }
            DriverBackend::Remote(client) => client.get_rpm(fan_idx),
            DriverBackend::WinIO(raw) => unsafe {
                (raw.healthy_table_set_fan_index)(fan_idx);
                // The EC needs a beat to latch the new fan index before the tach
                // registers (0x33/0x34) reflect that fan. The project's own probe
                // uses 20-30ms; 10ms was returning the previous fan's value.
                std::thread::sleep(std::time::Duration::from_millis(25));
                let rpm = (raw.healthy_table_fan_rpm)();
                if rpm < 0 {
                    return Err(DriverError::FanReadFailed {
                        fan_index: fan_idx,
                        value: rpm,
                    });
                }
                Ok(rpm as u32)
            },
        }
    }

    pub fn get_all_fan_rpms(&self) -> Result<Vec<u32>, DriverError> {
        // One round trip instead of N when talking to the helper.
        if let DriverBackend::Remote(client) = &self.backend {
            return client.get_rpms();
        }
        let mut rpms = Vec::with_capacity(self.fan_count as usize);
        for i in 0..self.fan_count {
            rpms.push(self.get_fan_rpm(i)?);
        }
        Ok(rpms)
    }

    pub fn get_cpu_temperature(&mut self) -> Result<u32, DriverError> {
        match &self.backend {
            DriverBackend::Acpi(acpi) => {
                if let Ok(Some(temp)) = acpi.get_cpu_temperature() {
                    self.last_known_temp = temp;
                    return Ok(temp);
                }
            }
            DriverBackend::Remote(client) => {
                if let Ok(temp) = client.get_temp() {
                    if (15..=115).contains(&temp) {
                        self.last_known_temp = temp;
                        return Ok(temp);
                    }
                }
            }
            DriverBackend::WinIO(raw) => {
                let raw_temp = unsafe { (raw.thermal_read_cpu_temperature)() as u32 };
                if (15..=115).contains(&raw_temp) {
                    self.last_known_temp = raw_temp;
                    return Ok(raw_temp);
                }
            }
        }

        if let Some(ref mut tr) = self.thermal_reader {
            if let Some(temp) = tr.read_temperature_c() {
                self.last_known_temp = temp;
                return Ok(temp);
            }
        }

        Ok(self.last_known_temp)
    }

    pub fn get_system_telemetry(&mut self) -> Result<SystemTelemetry, DriverError> {
        let cpu_temp_c = self.get_cpu_temperature()?;
        let mut fans = Vec::with_capacity(self.fan_count as usize);

        for i in 0..self.fan_count {
            let rpm = self.get_fan_rpm(i)?;
            fans.push(FanTelemetry {
                id: FanId(i),
                rpm,
                target_percent: None,
            });
        }

        Ok(SystemTelemetry {
            cpu_temp_c,
            fans,
        })
    }

    pub fn set_fan_duty(&mut self, fan_idx: u8, duty: FanPwm) -> Result<(), DriverError> {
        let percent = duty.to_percent();
        self.set_fan_percent(fan_idx, percent)
    }

    pub fn set_fan_percent(&mut self, fan_idx: u8, percent: u8) -> Result<(), DriverError> {
        self.validate_fan_index(fan_idx)?;
        if percent > 100 {
            return Err(DriverError::InvalidPercentage { percent });
        }

        match &self.backend {
            DriverBackend::Acpi(acpi) => {
                if percent == 0 {
                    acpi.reset_to_bios()?;
                    debug!("Fan #{} reset to BIOS control via ACPI", fan_idx);
                } else {
                    acpi.set_fan_percent(fan_idx, percent)?;
                    debug!("Fan #{} set to {}% via ACPI", fan_idx, percent);
                }
                Ok(())
            }
            DriverBackend::Remote(client) => {
                client.set_duty(fan_idx, percent)?;
                debug!("Fan #{} set to {}% over IPC", fan_idx, percent);
                Ok(())
            }
            DriverBackend::WinIO(raw) => {
                let pwm = FanPwm::from_percent(percent);
                unsafe {
                    (raw.healthy_table_set_fan_index)(fan_idx);
                    if pwm.0 > 0 {
                        (raw.healthy_table_set_fan_test_mode)(0x01u16);
                        (raw.healthy_table_set_fan_pwm_duty)(pwm.0 as i16);
                        debug!("Fan #{} set to PWM {} (Test Mode enabled)", fan_idx, pwm.0);
                    } else {
                        (raw.healthy_table_set_fan_test_mode)(0x00u16);
                        (raw.healthy_table_set_fan_pwm_duty)(0);
                        debug!("Fan #{} test mode disabled (BIOS control restored)", fan_idx);
                    }
                }
                Ok(())
            }
        }
    }

    pub fn set_all_fans_percent(&mut self, percent: u8) -> Result<(), DriverError> {
        if percent > 100 {
            return Err(DriverError::InvalidPercentage { percent });
        }

        match &self.backend {
            DriverBackend::Acpi(acpi) => {
                if percent == 0 {
                    acpi.reset_to_bios()?;
                } else {
                    for i in 0..self.fan_count {
                        acpi.set_fan_percent(i, percent)?;
                    }
                }
                Ok(())
            }
            DriverBackend::Remote(client) => client.set_all(percent),
            DriverBackend::WinIO(_) => {
                let pwm = FanPwm::from_percent(percent);
                for i in 0..self.fan_count {
                    self.set_fan_duty(i, pwm)?;
                    std::thread::sleep(std::time::Duration::from_millis(15));
                }
                Ok(())
            }
        }
    }

    pub fn reset_fan_to_bios(&mut self, fan_idx: u8) -> Result<(), DriverError> {
        self.validate_fan_index(fan_idx)?;
        match &self.backend {
            DriverBackend::Acpi(acpi) => {
                acpi.reset_to_bios()?;
            }
            DriverBackend::Remote(client) => {
                client.reset_one(fan_idx)?;
            }
            DriverBackend::WinIO(raw) => unsafe {
                (raw.healthy_table_set_fan_index)(fan_idx);
                (raw.healthy_table_set_fan_test_mode)(0x00u16);
                (raw.healthy_table_set_fan_pwm_duty)(0);
            },
        }
        info!("Fan #{} reset to BIOS control", fan_idx);
        Ok(())
    }

    pub fn reset_all_to_bios(&mut self) -> Result<(), DriverError> {
        match &self.backend {
            DriverBackend::Acpi(acpi) => {
                acpi.reset_to_bios()?;
            }
            DriverBackend::Remote(client) => {
                client.reset_all()?;
            }
            DriverBackend::WinIO(_) => {
                for i in 0..self.fan_count {
                    let _ = self.reset_fan_to_bios(i);
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
        info!("All fans safely reset to BIOS control.");
        Ok(())
    }

    fn validate_fan_index(&self, fan_idx: u8) -> Result<(), DriverError> {
        if fan_idx >= self.fan_count {
            Err(DriverError::InvalidFanIndex {
                index: fan_idx,
                max_fans: self.fan_count,
            })
        } else {
            Ok(())
        }
    }
}

impl Drop for AsusDriver {
    fn drop(&mut self) {
        info!("AsusDriver dropping: safely releasing hardware to BIOS control...");
        match &mut self.backend {
            DriverBackend::Acpi(acpi) => {
                let _ = acpi.reset_to_bios();
            }
            DriverBackend::Remote(_) => {
                // Nothing to release locally. Dropping the pipe is the signal:
                // the helper detects the disconnect and returns the fans to
                // BIOS control itself.
            }
            DriverBackend::WinIO(raw) => {
                for i in 0..self.fan_count {
                    unsafe {
                        (raw.healthy_table_set_fan_index)(i);
                        (raw.healthy_table_set_fan_test_mode)(0x00u16);
                    }
                }
                unsafe {
                    (raw.shutdown_win_io)();
                }
            }
        }
        info!("Hardware shutdown complete.");
    }
}

/// A cloneable, thread-safe handle to AsusDriver.
/// All driver transactions are strictly serialized through the Mutex.
#[derive(Clone)]
pub struct SafeAsusDriver {
    inner: Arc<Mutex<AsusDriver>>,
}

impl SafeAsusDriver {
    pub fn new() -> Result<Self, DriverError> {
        let driver = AsusDriver::new()?;
        Ok(Self {
            inner: Arc::new(Mutex::new(driver)),
        })
    }

    /// Attaches to the SYSTEM helper running alongside this process.
    pub fn connect(pipe_name: &str) -> Result<Self, DriverError> {
        let driver = AsusDriver::connect(pipe_name)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(driver)),
        })
    }

    pub fn lock(&self) -> parking_lot::MutexGuard<'_, AsusDriver> {
        self.inner.lock()
    }
}
