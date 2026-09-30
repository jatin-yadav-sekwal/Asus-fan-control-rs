use std::path::{Path, PathBuf};
use tracing::{debug, info};
use crate::error::DriverError;

pub type FnInitializeWinIo = unsafe extern "system" fn();
pub type FnShutdownWinIo = unsafe extern "system" fn();
pub type FnHealthyTableFanCounts = unsafe extern "system" fn() -> i32;
pub type FnHealthyTableSetFanIndex = unsafe extern "system" fn(u8);
pub type FnHealthyTableFanRPM = unsafe extern "system" fn() -> i32;
pub type FnHealthyTableSetFanTestMode = unsafe extern "system" fn(u16);
pub type FnHealthyTableSetFanPwmDuty = unsafe extern "system" fn(i16);
pub type FnThermalReadCpuTemperature = unsafe extern "system" fn() -> u64;

pub struct RawAsusWinIO {
    _lib: libloading::Library,
    pub initialize_win_io: FnInitializeWinIo,
    pub shutdown_win_io: FnShutdownWinIo,
    pub healthy_table_fan_counts: FnHealthyTableFanCounts,
    pub healthy_table_set_fan_index: FnHealthyTableSetFanIndex,
    pub healthy_table_fan_rpm: FnHealthyTableFanRPM,
    pub healthy_table_set_fan_test_mode: FnHealthyTableSetFanTestMode,
    pub healthy_table_set_fan_pwm_duty: FnHealthyTableSetFanPwmDuty,
    pub thermal_read_cpu_temperature: FnThermalReadCpuTemperature,
    pub loaded_from: PathBuf,
}

impl RawAsusWinIO {
    pub fn load() -> Result<Self, DriverError> {
        let candidates = Self::discover_dll_paths();
        for path in &candidates {
            if path.exists() {
                debug!("Attempting to load AsusWinIO64.dll from: {:?}", path);
                match unsafe { Self::load_from_path(path) } {
                    Ok(raw) => {
                        info!("Successfully loaded AsusWinIO64 from: {:?}", path);
                        return Ok(raw);
                    }
                    Err(err) => {
                        debug!("Failed to load from {:?}: {}", path, err);
                    }
                }
            }
        }

        Err(DriverError::DllNotFound(candidates))
    }

    pub unsafe fn load_from_path(path: &Path) -> Result<Self, DriverError> {
        let lib = libloading::Library::new(path)?;

        let initialize_win_io: FnInitializeWinIo = *lib
            .get(b"InitializeWinIo\0")
            .map_err(|e| DriverError::SymbolNotFound {
                symbol: "InitializeWinIo",
                source: e,
            })?;

        let shutdown_win_io: FnShutdownWinIo = *lib
            .get(b"ShutdownWinIo\0")
            .map_err(|e| DriverError::SymbolNotFound {
                symbol: "ShutdownWinIo",
                source: e,
            })?;

        let healthy_table_fan_counts: FnHealthyTableFanCounts = *lib
            .get(b"HealthyTable_FanCounts\0")
            .map_err(|e| DriverError::SymbolNotFound {
                symbol: "HealthyTable_FanCounts",
                source: e,
            })?;

        let healthy_table_set_fan_index: FnHealthyTableSetFanIndex = *lib
            .get(b"HealthyTable_SetFanIndex\0")
            .map_err(|e| DriverError::SymbolNotFound {
                symbol: "HealthyTable_SetFanIndex",
                source: e,
            })?;

        let healthy_table_fan_rpm: FnHealthyTableFanRPM = *lib
            .get(b"HealthyTable_FanRPM\0")
            .map_err(|e| DriverError::SymbolNotFound {
                symbol: "HealthyTable_FanRPM",
                source: e,
            })?;

        let healthy_table_set_fan_test_mode: FnHealthyTableSetFanTestMode = *lib
            .get(b"HealthyTable_SetFanTestMode\0")
            .map_err(|e| DriverError::SymbolNotFound {
                symbol: "HealthyTable_SetFanTestMode",
                source: e,
            })?;

        let healthy_table_set_fan_pwm_duty: FnHealthyTableSetFanPwmDuty = *lib
            .get(b"HealthyTable_SetFanPwmDuty\0")
            .map_err(|e| DriverError::SymbolNotFound {
                symbol: "HealthyTable_SetFanPwmDuty",
                source: e,
            })?;

        let thermal_read_cpu_temperature: FnThermalReadCpuTemperature = *lib
            .get(b"Thermal_Read_Cpu_Temperature\0")
            .map_err(|e| DriverError::SymbolNotFound {
                symbol: "Thermal_Read_Cpu_Temperature",
                source: e,
            })?;

        Ok(Self {
            _lib: lib,
            initialize_win_io,
            shutdown_win_io,
            healthy_table_fan_counts,
            healthy_table_set_fan_index,
            healthy_table_fan_rpm,
            healthy_table_set_fan_test_mode,
            healthy_table_set_fan_pwm_duty,
            thermal_read_cpu_temperature,
            loaded_from: path.to_path_buf(),
        })
    }

    fn discover_dll_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();

        // 1. Explicit environment variable
        if let Ok(env_path) = std::env::var("ASUS_WINIO_PATH") {
            paths.push(PathBuf::from(env_path));
        }

        // 2. Alongside the current executable
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                paths.push(exe_dir.join("AsusWinIO64.dll"));
                paths.push(exe_dir.join("..").join("AsusWinIO64.dll"));
            }
        }

        // 3. Current working directory
        if let Ok(cwd) = std::env::current_dir() {
            paths.push(cwd.join("AsusWinIO64.dll"));
            paths.push(cwd.join("..").join("AsusWinIO64.dll"));
            paths.push(cwd.join("AsusFanControl").join("AsusWinIO64.dll"));
        }

        // 4. Cloned project directory fallback
        paths.push(PathBuf::from(r"c:\Users\Asus\Downloads\AsusFanControl\bin\AsusWinIO64.dll"));
        paths.push(PathBuf::from(r"c:\Users\Asus\Downloads\AsusFanControl\AsusWinIO64.dll"));
        paths.push(PathBuf::from(r"c:\Users\Asus\Downloads\AsusFanControl\AsusFanControl\AsusFanControl\AsusWinIO64.dll"));

        // 5. ASUS DriverStore repository
        let driver_store = PathBuf::from(r"C:\Windows\System32\DriverStore\FileRepository");
        if driver_store.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&driver_store) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("asussci2.inf_amd64_") {
                        let candidate = entry.path().join("ASUSSystemAnalysis").join("AsusWinIO64.dll");
                        paths.push(candidate);
                    }
                }
            }
        }

        paths
    }
}
