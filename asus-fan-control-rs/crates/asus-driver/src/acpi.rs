use std::ffi::c_void;
use tracing::{debug, info, warn};

use crate::error::DriverError;

const ATKACPI_DEVICE: &str = r"\\.\ATKACPI";
const IOCTL_ASUS_CONTROL: u32 = 0x0022240C;

// ACPI Methods
const METHOD_DSTS: u32 = 0x53545344; // 'DSTS'
const METHOD_DEVS: u32 = 0x53564544; // 'DEVS'
const METHOD_INIT: u32 = 0x54494E49; // 'INIT'

// ACPI Device IDs (from G-Helper / ASUS System Control Interface)
pub const ACPI_CPU_FAN: u32 = 0x00110013;
pub const ACPI_GPU_FAN: u32 = 0x00110014;
pub const ACPI_MID_FAN: u32 = 0x00110031;

pub const ACPI_TEMP_CPU: u32 = 0x00120094;
pub const ACPI_TEMP_GPU: u32 = 0x00120097;

pub const ACPI_DEVS_CPU_FAN: u32 = 0x00110022;
pub const ACPI_DEVS_GPU_FAN: u32 = 0x00110023;

pub const ACPI_DEVS_CPU_FAN_CURVE: u32 = 0x00110024;
pub const ACPI_DEVS_GPU_FAN_CURVE: u32 = 0x00110025;
pub const ACPI_DEVS_MID_FAN_CURVE: u32 = 0x00110032;

pub const ACPI_PERFORMANCE_MODE: u32 = 0x00120075;
pub const ACPI_VIVOBOOK_MODE: u32 = 0x00110019;

// Windows API constants
const GENERIC_READ: u32 = 0x80000000;
const GENERIC_WRITE: u32 = 0x40000000;
const FILE_SHARE_READ: u32 = 0x00000001;
const FILE_SHARE_WRITE: u32 = 0x00000002;
const OPEN_EXISTING: u32 = 3;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x00000080;
const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;

#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        lpFileName: *const u16,
        dwDesiredAccess: u32,
        dwShareMode: u32,
        lpSecurityAttributes: *mut c_void,
        dwCreationDisposition: u32,
        dwFlagsAndAttributes: u32,
        hTemplateFile: *mut c_void,
    ) -> *mut c_void;

    fn DeviceIoControl(
        hDevice: *mut c_void,
        dwIoControlCode: u32,
        lpInBuffer: *const c_void,
        nInBufferSize: u32,
        lpOutBuffer: *mut c_void,
        nOutBufferSize: u32,
        lpBytesReturned: *mut u32,
        lpOverlapped: *mut c_void,
    ) -> i32;

    fn CloseHandle(hObject: *mut c_void) -> i32;
    fn GetLastError() -> u32;
}

pub struct AcpiDevice {
    handle: *mut c_void,
}

unsafe impl Send for AcpiDevice {}
unsafe impl Sync for AcpiDevice {}

impl AcpiDevice {
    /// Attempts to open the ASUS ACPI control device (`\\.\ATKACPI`).
    pub fn open() -> Result<Self, DriverError> {
        let wide_path: Vec<u16> = ATKACPI_DEVICE
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let handle = unsafe {
            CreateFileW(
                wide_path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };

        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            let err = unsafe { GetLastError() };
            warn!("Failed to open \\\\.\\ATKACPI device, win error: {}", err);
            return Err(DriverError::Io(std::io::Error::from_raw_os_error(err as i32)));
        }

        info!("Successfully opened \\\\.\\ATKACPI handle: {:?}", handle);

        let dev = Self { handle };
        // Perform INIT handshake
        let _ = dev.call_method(METHOD_INIT, &[0u8; 8]);
        Ok(dev)
    }

    /// Invokes an ACPI method via `DeviceIoControl` using IOCTL `0x0022240C`.
    pub fn call_method(&self, method_id: u32, args: &[u8]) -> Result<[u8; 16], DriverError> {
        let mut buffer = Vec::with_capacity(8 + args.len());
        buffer.extend_from_slice(&method_id.to_le_bytes());
        buffer.extend_from_slice(&(args.len() as u32).to_le_bytes());
        buffer.extend_from_slice(args);

        let mut out_buffer = [0u8; 16];
        let mut bytes_returned = 0u32;

        let ret = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_ASUS_CONTROL,
                buffer.as_ptr() as *const c_void,
                buffer.len() as u32,
                out_buffer.as_mut_ptr() as *mut c_void,
                out_buffer.len() as u32,
                &mut bytes_returned,
                std::ptr::null_mut(),
            )
        };

        if ret == 0 {
            let err = unsafe { GetLastError() };
            debug!("DeviceIoControl(0x{:X}) failed with win error: {}", method_id, err);
            return Err(DriverError::Io(std::io::Error::from_raw_os_error(err as i32)));
        }

        Ok(out_buffer)
    }

    /// Query device status (DSTS method). Returns raw value minus 65536.
    pub fn device_get(&self, device_id: u32) -> Result<i32, DriverError> {
        let mut args = [0u8; 8];
        args[0..4].copy_from_slice(&device_id.to_le_bytes());
        let status = self.call_method(METHOD_DSTS, &args)?;
        let raw = i32::from_le_bytes(status[0..4].try_into().unwrap());
        Ok(raw - 65536)
    }

    /// Set device status (DEVS method) with a 32-bit integer status.
    pub fn device_set(&self, device_id: u32, status: i32) -> Result<i32, DriverError> {
        let mut args = [0u8; 8];
        args[0..4].copy_from_slice(&device_id.to_le_bytes());
        args[4..8].copy_from_slice(&status.to_le_bytes());
        let res = self.call_method(METHOD_DEVS, &args)?;
        Ok(i32::from_le_bytes(res[0..4].try_into().unwrap()))
    }

    /// Set device status (DEVS method) with arbitrary byte payload.
    pub fn device_set_bytes(&self, device_id: u32, params: &[u8]) -> Result<i32, DriverError> {
        let mut args = Vec::with_capacity(4 + params.len());
        args.extend_from_slice(&device_id.to_le_bytes());
        args.extend_from_slice(params);
        let res = self.call_method(METHOD_DEVS, &args)?;
        Ok(i32::from_le_bytes(res[0..4].try_into().unwrap()))
    }

    /// Reads fan RPM for a given fan index (0 = CPU, 1 = GPU, 2 = Mid).
    /// Returns real-time RPM, or None if fan is unsupported or idle.
    pub fn get_fan_rpm(&self, fan_idx: u8) -> Result<Option<u32>, DriverError> {
        let endpoint = match fan_idx {
            0 => ACPI_CPU_FAN,
            1 => ACPI_GPU_FAN,
            2 => ACPI_MID_FAN,
            _ => return Ok(None),
        };

        let raw = self.device_get(endpoint)?;
        let fan = raw & 0xFFFF;
        // In G-Helper: fan > 120 or (fan == 0 && raw < 0) means invalid/unsupported
        if fan > 120 || (fan == 0 && raw < 0) {
            return Ok(None);
        }

        // Each unit corresponds to 100 RPM
        let rpm = (fan as u32) * 100;
        Ok(Some(rpm))
    }

    /// Reads CPU temperature in °C directly from EC.
    pub fn get_cpu_temperature(&self) -> Result<Option<u32>, DriverError> {
        let raw = self.device_get(ACPI_TEMP_CPU)?;
        let temp = (raw & 0xFFFF) as u32;
        if (15..=115).contains(&temp) {
            Ok(Some(temp))
        } else {
            Ok(None)
        }
    }

    /// Reads GPU temperature in °C directly from EC.
    pub fn get_gpu_temperature(&self) -> Result<Option<u32>, DriverError> {
        let raw = self.device_get(ACPI_TEMP_GPU)?;
        let temp = (raw & 0xFFFF) as u32;
        if (15..=115).contains(&temp) {
            Ok(Some(temp))
        } else {
            Ok(None)
        }
    }

    /// Sets a 16-byte fan curve (8 temp points, 8 fan percent points).
    pub fn set_fan_curve(&self, fan_idx: u8, speeds: &[u8; 8]) -> Result<(), DriverError> {
        let endpoint = match fan_idx {
            0 => ACPI_DEVS_CPU_FAN_CURVE,
            1 => ACPI_DEVS_GPU_FAN_CURVE,
            2 => ACPI_DEVS_MID_FAN_CURVE,
            _ => return Ok(()),
        };

        // Standard temperature curve points: 30°C to 100°C in ~10°C increments
        let temps: [u8; 8] = [30, 40, 50, 60, 70, 80, 90, 100];
        let mut curve = [0u8; 16];
        curve[0..8].copy_from_slice(&temps);
        for i in 0..8 {
            curve[8 + i] = speeds[i].min(100);
        }

        let res = self.device_set_bytes(endpoint, &curve)?;
        debug!("ACPI SetFanCurve fan #{} result: {}", fan_idx, res);

        // Also update Fan Range (min/max PWM)
        let min_pwm = (speeds[0] as f32 * 2.55).round() as u8;
        let max_pwm = (speeds[7] as f32 * 2.55).round() as u8;
        let range = [min_pwm, max_pwm];

        let range_endpoint = match fan_idx {
            0 => Some(ACPI_DEVS_CPU_FAN),
            1 => Some(ACPI_DEVS_GPU_FAN),
            _ => None,
        };

        if let Some(rep) = range_endpoint {
            let rres = self.device_set_bytes(rep, &range)?;
            debug!("ACPI SetFanRange fan #{} result: {}", fan_idx, rres);
        }

        Ok(())
    }

    /// Sets fan manual speed to a fixed percentage across all temperature bands.
    pub fn set_fan_percent(&self, fan_idx: u8, percent: u8) -> Result<(), DriverError> {
        let clamped = percent.min(100);
        let speeds = [clamped; 8];
        self.set_fan_curve(fan_idx, &speeds)
    }

    /// Resets fans back to BIOS automatic management by restoring the Performance Mode profile.
    pub fn reset_to_bios(&self) -> Result<(), DriverError> {
        // Reset custom curve buffers
        let zero_curve = [0u8; 16];
        let _ = self.device_set_bytes(ACPI_DEVS_CPU_FAN_CURVE, &zero_curve);
        let _ = self.device_set_bytes(ACPI_DEVS_GPU_FAN_CURVE, &zero_curve);
        let _ = self.device_set_bytes(ACPI_DEVS_MID_FAN_CURVE, &zero_curve);

        // Reapply Performance Mode 0 (Balanced / standard factory control)
        let res = self.device_set(ACPI_PERFORMANCE_MODE, 0)?;
        if res != 1 {
            let _ = self.device_set(ACPI_VIVOBOOK_MODE, 0);
        }
        info!("All fans reset to BIOS control via ACPI.");
        Ok(())
    }
}

impl Drop for AcpiDevice {
    fn drop(&mut self) {
        if self.handle != INVALID_HANDLE_VALUE && !self.handle.is_null() {
            let _ = self.reset_to_bios();
            unsafe {
                CloseHandle(self.handle);
            }
            debug!("Closed ATKACPI handle");
        }
    }
}
