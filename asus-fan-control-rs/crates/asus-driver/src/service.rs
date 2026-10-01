//! Preflight repair for the `AsusSAIO` kernel driver service.
//!
//! `AsusWinIO64.dll` creates/starts this service itself, but a previous run that
//! was killed mid-flight can leave the service `Disabled` or stuck in
//! `Stop Pending`. In that state every `HealthyTable_*` export returns `-1`,
//! which the old code silently turned into "0 RPM". We normalise the service
//! before handing over to the DLL.
//!
//! The DLL also records an absolute path to `.\AsusSAIO.sys` next to itself, so
//! removing or relocating the application leaves the service pointing at a file
//! that no longer exists - and the next run then has no driver to start. The
//! image path is therefore repaired as part of the same preflight.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::time::{Duration, Instant};

use tracing::{info, warn};

pub const DRIVER_SERVICE: &str = "AsusSAIO";

const SC_MANAGER_CONNECT: u32 = 0x0001;
const SERVICE_QUERY_CONFIG: u32 = 0x0001;
const SERVICE_CHANGE_CONFIG: u32 = 0x0002;
const SERVICE_QUERY_STATUS: u32 = 0x0004;
const SERVICE_START: u32 = 0x0010;
const SERVICE_STOP: u32 = 0x0020;

const SERVICE_KERNEL_DRIVER: u32 = 0x0000_0001;
const SERVICE_DEMAND_START: u32 = 0x0000_0003;
const SERVICE_CONTROL_STOP: u32 = 0x0000_0001;

const SERVICE_STOPPED: u32 = 1;
const SERVICE_START_PENDING: u32 = 2;
const SERVICE_STOP_PENDING: u32 = 3;
const SERVICE_RUNNING: u32 = 4;

const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_USER: u32 = 1;

#[repr(C)]
struct ServiceStatus {
    dw_service_type: u32,
    dw_current_state: u32,
    dw_controls_accepted: u32,
    dw_win32_exit_code: u32,
    dw_service_specific_exit_code: u32,
    dw_check_point: u32,
    dw_wait_hint: u32,
}

/// `QUERY_SERVICE_CONFIGW`. The four string members point *into* the buffer the
/// API filled, so they are only valid while that buffer is alive.
#[repr(C)]
struct QueryServiceConfig {
    dw_service_type: u32,
    dw_start_type: u32,
    dw_error_control: u32,
    lp_binary_path_name: *mut u16,
    lp_load_order_group: *mut u16,
    dw_tag_id: u32,
    lp_dependencies: *mut u16,
    lp_service_start_name: *mut u16,
    lp_display_name: *mut u16,
}

#[repr(C)]
struct SidAndAttributes {
    sid: *mut c_void,
    attributes: u32,
}

#[repr(C)]
struct TokenUser {
    user: SidAndAttributes,
}

#[link(name = "advapi32")]
extern "system" {
    fn OpenSCManagerW(
        lp_machine_name: *const u16,
        lp_database_name: *const u16,
        dw_desired_access: u32,
    ) -> *mut c_void;

    fn OpenServiceW(
        h_service_manager: *mut c_void,
        lp_service_name: *const u16,
        dw_desired_access: u32,
    ) -> *mut c_void;

    fn ChangeServiceConfigW(
        h_service: *mut c_void,
        dw_service_type: u32,
        dw_start_type: u32,
        dw_error_control: u32,
        lp_binary_path_name: *const u16,
        lp_load_order_group: *const u16,
        lpdw_tag_id: *mut u32,
        lp_dependencies: *const u16,
        lp_service_start_name: *const u16,
        lp_password: *const u16,
        lp_display_name: *const u16,
    ) -> i32;

    fn QueryServiceStatus(h_service: *mut c_void, lp_service_status: *mut ServiceStatus) -> i32;
    fn QueryServiceConfigW(
        h_service: *mut c_void,
        lp_service_config: *mut QueryServiceConfig,
        cb_buf_size: u32,
        pcb_bytes_needed: *mut u32,
    ) -> i32;
    fn ControlService(
        h_service: *mut c_void,
        dw_control: u32,
        lp_service_status: *mut ServiceStatus,
    ) -> i32;
    fn CloseServiceHandle(h_object: *mut c_void) -> i32;

    fn OpenProcessToken(
        process_handle: *mut c_void,
        desired_access: u32,
        token_handle: *mut *mut c_void,
    ) -> i32;

    fn GetTokenInformation(
        token_handle: *mut c_void,
        token_information_class: u32,
        token_information: *mut c_void,
        token_information_length: u32,
        return_length: *mut u32,
    ) -> i32;

}

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcess() -> *mut c_void;
    fn CloseHandle(hObject: *mut c_void) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Copies a NUL-terminated UTF-16 string written into our buffer by
/// `QueryServiceConfigW`. The API guarantees termination; the cap only bounds
/// the read if a malformed buffer ever comes back.
unsafe fn wide_string(p: *const u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let mut len = 0usize;
    while len < 4096 && *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf16(std::slice::from_raw_parts(p, len)).ok()
}

/// The `BINARY_PATH_NAME` of `AsusSAIO`, or `None` when it cannot be read.
fn query_image_path(service: *mut c_void) -> Option<String> {
    let mut needed: u32 = 0;
    unsafe {
        QueryServiceConfigW(service, std::ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 {
        return None;
    }

    let mut buf = vec![0u8; needed as usize];
    let ok = unsafe {
        QueryServiceConfigW(
            service,
            buf.as_mut_ptr() as *mut QueryServiceConfig,
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return None;
    }

    let cfg = unsafe { &*(buf.as_ptr() as *const QueryServiceConfig) };
    unsafe { wide_string(cfg.lp_binary_path_name) }
}

/// This install's copy of `AsusSAIO.sys`, the file the DLL opens relative to
/// itself.
fn own_driver_copy() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let candidate = exe.parent()?.join("AsusSAIO.sys");
    candidate
        .exists()
        .then(|| candidate.to_string_lossy().into_owned())
}

/// Points `AsusSAIO` back at a driver file that actually exists.
///
/// Non-destructive by design: a service whose image path still resolves is left
/// exactly as it is, and the repoint only happens when our own copy is present.
fn repair_image_path(service: *mut c_void) {
    let configured = match query_image_path(service) {
        Some(raw) => raw.trim().trim_matches('"').to_string(),
        None => {
            warn!("AsusSAIO image path unreadable; leaving it untouched");
            return;
        }
    };
    if configured.is_empty() {
        return;
    }
    if std::path::Path::new(&configured).exists() {
        info!("AsusSAIO image path still valid: {}", configured);
        return;
    }

    let own = match own_driver_copy() {
        Some(p) => p,
        None => {
            warn!(
                "AsusSAIO image path '{}' is gone and no local AsusSAIO.sys was \
                 found to repoint at",
                configured
            );
            return;
        }
    };

    let wide_own = wide(&own);
    let changed = unsafe {
        ChangeServiceConfigW(
            service,
            SERVICE_KERNEL_DRIVER,
            SERVICE_DEMAND_START,
            1, // ERROR_CONTROL_NORMAL
            wide_own.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if changed == 0 {
        warn!(
            "ChangeServiceConfigW(AsusSAIO, binary path) failed (err {})",
            unsafe { kernel32_get_last_error() }
        );
    } else {
        info!(
            "AsusSAIO image path repaired: '{}' -> '{}'",
            configured, own
        );
    }
}

/// `true` when the current process runs as `NT AUTHORITY\SYSTEM`.
///
/// `\\.\AsusSAIO` only accepts SYSTEM — an elevated administrator is rejected
/// and every `HealthyTable_*` call returns `-1`.
///
/// The SID is decoded directly from the `SID` byte layout
/// (`[revision][count][6-byte authority][DWORD sub-auths...]`) instead of
/// `CreateWellKnownSid` + `EqualSid`, which reported `false` even for a token
/// that `whoami /user` showed as `S-1-5-18`.
pub fn is_local_system() -> bool {
    let Some(sid) = token_user_sid_bytes() else {
        return false;
    };

    // S-1-5-18 = NT AUTHORITY\SYSTEM
    sid.len() >= 12
        && sid[0] == 1 // revision
        && sid[1] == 1 // one sub-authority
        && sid[2..8] == [0, 0, 0, 0, 0, 5] // NT authority
        && u32::from_le_bytes([sid[8], sid[9], sid[10], sid[11]]) == 18
}

/// Human-readable form of the current process token's user SID, for logs.
///
/// `USERNAME` is useless here: a SYSTEM process reports `HOSTNAME$`.
pub fn token_sid_string() -> String {
    let Some(sid) = token_user_sid_bytes() else {
        return "?".to_string();
    };
    if sid.len() < 8 {
        return "?".to_string();
    }

    let mut authority: u64 = 0;
    for byte in &sid[2..8] {
        authority = (authority << 8) | u64::from(*byte);
    }
    let mut out = format!("S-{}-{}", sid[0], authority);
    for i in 0..usize::from(sid[1]) {
        let off = 8 + i * 4;
        if off + 4 > sid.len() {
            break;
        }
        out.push('-');
        out.push_str(&u32::from_le_bytes(sid[off..off + 4].try_into().unwrap()).to_string());
    }
    out
}

/// Returns the raw bytes of the current process token's user SID.
pub(crate) fn token_user_sid_bytes() -> Option<Vec<u8>> {
    unsafe {
        let mut token: *mut c_void = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let result = (|| {
            let mut needed = 0u32;
            GetTokenInformation(token, TOKEN_USER, std::ptr::null_mut(), 0, &mut needed);
            if needed == 0 {
                return None;
            }

            // u64 slots keep the buffer aligned for the TOKEN_USER pointer field.
            let mut buf = vec![0u64; needed.div_ceil(8) as usize];
            let ok = GetTokenInformation(
                token,
                TOKEN_USER,
                buf.as_mut_ptr() as *mut c_void,
                needed,
                &mut needed,
            );
            if ok == 0 {
                return None;
            }

            let sid = (*(buf.as_ptr() as *const TokenUser)).user.sid as *const u8;
            if sid.is_null() {
                return None;
            }

            let count = *sid.add(1) as usize;
            let len = 8 + count.saturating_mul(4);
            Some(std::slice::from_raw_parts(sid, len).to_vec())
        })();
        CloseHandle(token);
        result
    }
}

fn open_sc_manager() -> Option<*mut c_void> {
    let handle = unsafe { OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT) };
    if handle.is_null() {
        None
    } else {
        Some(handle)
    }
}

fn query_state(service: *mut c_void) -> Option<(u32, u32)> {
    let mut status = ServiceStatus {
        dw_service_type: 0,
        dw_current_state: 0,
        dw_controls_accepted: 0,
        dw_win32_exit_code: 0,
        dw_service_specific_exit_code: 0,
        dw_check_point: 0,
        dw_wait_hint: 0,
    };
    let ok = unsafe { QueryServiceStatus(service, &mut status) };
    if ok == 0 {
        None
    } else {
        Some((status.dw_current_state, status.dw_win32_exit_code))
    }
}

fn state_name(state: u32) -> &'static str {
    match state {
        SERVICE_STOPPED => "STOPPED",
        SERVICE_START_PENDING => "START_PENDING",
        SERVICE_STOP_PENDING => "STOP_PENDING",
        SERVICE_RUNNING => "RUNNING",
        _ => "OTHER",
    }
}

fn wait_until_settled(service: *mut c_void, allowed: &[u32], timeout: Duration) -> Option<u32> {
    let deadline = Instant::now() + timeout;
    loop {
        let (state, _) = query_state(service)?;
        if allowed.contains(&state) {
            return Some(state);
        }
        if Instant::now() >= deadline {
            return Some(state);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Best-effort normalisation of the `AsusSAIO` service so `InitializeWinIo()`
/// can actually start the driver.
///
/// Never fails hard — the caller validates the result functionally via
/// `HealthyTable_FanCounts()`.
pub fn preflight_repair() {
    let sid = token_sid_string();
    if !is_local_system() {
        warn!(
            "Not running as NT AUTHORITY\\SYSTEM (token SID: {}). \\\\.\\{} usually rejects \
             non-SYSTEM callers, so HealthyTable_* will return -1. Start the SYSTEM helper \
             instead of opening the driver directly.",
            sid,
            DRIVER_SERVICE
        );
        return;
    }
    info!("Token SID {} is NT AUTHORITY\\SYSTEM; preflight OK", sid);

    let scm = match open_sc_manager() {
        Some(h) => h,
        None => {
            warn!("OpenSCManagerW failed (err {}); skipping AsusSAIO preflight", unsafe {
                kernel32_get_last_error()
            });
            return;
        }
    };

    let svc_name = wide(DRIVER_SERVICE);
    let service = unsafe { OpenServiceW(scm, svc_name.as_ptr(), SERVICE_QUERY_CONFIG | SERVICE_CHANGE_CONFIG | SERVICE_QUERY_STATUS | SERVICE_START | SERVICE_STOP) };
    if service.is_null() {
        info!("AsusSAIO service not installed yet; AsusWinIO64.dll will create it.");
        unsafe { CloseServiceHandle(scm) };
        return;
    }

    let (state, win32) = match query_state(service) {
        Some(v) => v,
        None => {
            unsafe {
                CloseServiceHandle(service);
                CloseServiceHandle(scm);
            }
            return;
        }
    };
    info!("AsusSAIO preflight: state={} win32_exit={}", state_name(state), win32);

    // 0. An image path left behind by an uninstall or a moved install makes
    //    StartService fail with 2 (file not found) and the DLL never retries.
    repair_image_path(service);

    // 1. A driver wedged in STOP_PENDING (usually because another fan-control
    //    process still holds \\.\AsusSAIO) blocks StartService forever.
    if state == SERVICE_STOP_PENDING {
        warn!("AsusSAIO is stuck STOP_PENDING; waiting for it to settle...");
        if let Some(final_state) = wait_until_settled(service, &[SERVICE_STOPPED, SERVICE_RUNNING], Duration::from_secs(6)) {
            if final_state == SERVICE_STOP_PENDING {
                warn!(
                    "AsusSAIO still STOP_PENDING after 6s — another process (AsusFanControlGUI?) \
                     is holding \\\\.\\{}. Close it and retry.",
                    DRIVER_SERVICE
                );
            }
        }
        let _ = unsafe { ControlService(service, SERVICE_CONTROL_STOP, &mut ServiceStatus {
            dw_service_type: 0,
            dw_current_state: 0,
            dw_controls_accepted: 0,
            dw_win32_exit_code: 0,
            dw_service_specific_exit_code: 0,
            dw_check_point: 0,
            dw_wait_hint: 0,
        }) };
        let _ = wait_until_settled(service, &[SERVICE_STOPPED], Duration::from_secs(4));
    }

    // 2. A DISABLED start type makes StartServiceW fail with 1058 and the DLL
    //    gives up without any error surface.
    let changed = unsafe {
        ChangeServiceConfigW(
            service,
            SERVICE_KERNEL_DRIVER,
            SERVICE_DEMAND_START,
            1, // ERROR_CONTROL_NORMAL
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if changed == 0 {
        warn!(
            "ChangeServiceConfigW(AsusSAIO, demand start) failed (err {})",
            unsafe { kernel32_get_last_error() }
        );
    } else {
        info!("AsusSAIO start type set to DEMAND_START");
    }

    unsafe {
        CloseServiceHandle(service);
        CloseServiceHandle(scm);
    }
}

#[link(name = "kernel32")]
extern "system" {
    // Maps the OS error code from GetLastError.
    #[link_name = "GetLastError"]
    fn kernel32_get_last_error() -> u32;
}

// ---------------------------------------------------------------------------
// Standalone bootstrap: UAC elevation, then a silent relaunch as SYSTEM.
// ---------------------------------------------------------------------------

const TOKEN_ELEVATION: u32 = 20;
const SW_SHOWNORMAL: i32 = 1;
const MB_OK: u32 = 0;
const MB_ICONERROR: u32 = 0x0000_0010;

#[repr(C)]
struct TokenElevation {
    token_is_elevated: u32,
}

#[link(name = "shell32")]
extern "system" {
    fn ShellExecuteW(
        hwnd: *mut c_void,
        lp_operation: *const u16,
        lp_file: *const u16,
        lp_parameters: *const u16,
        lp_directory: *const u16,
        n_show_cmd: i32,
    ) -> *mut c_void;
}

#[link(name = "user32")]
extern "system" {
    fn MessageBoxW(
        hwnd: *mut c_void,
        lp_text: *const u16,
        lp_caption: *const u16,
        u_type: u32,
    ) -> i32;
}

/// `true` when the process token carries an elevated (administrator) UAC
/// elevation type. Creating a scheduled task that runs as `NT AUTHORITY\SYSTEM`
/// is an administrator-only operation, so the GUI needs this before it can
/// start the SYSTEM helper.
pub fn is_elevated() -> bool {
    unsafe {
        let mut token: *mut c_void = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TokenElevation {
            token_is_elevated: 0,
        };
        let mut needed = 0u32;
        let ok = GetTokenInformation(
            token,
            TOKEN_ELEVATION,
            &mut elevation as *mut TokenElevation as *mut c_void,
            std::mem::size_of::<TokenElevation>() as u32,
            &mut needed,
        );
        CloseHandle(token);
        ok != 0 && elevation.token_is_elevated != 0
    }
}

/// Modal error dialog — the app is a GUI subsystem binary, so there is no
/// console to report bootstrap failures on.
pub fn show_error(title: &str, text: &str) {
    let t = wide(title);
    let m = wide(text);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            m.as_ptr(),
            t.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

/// Relaunches the current executable elevated (triggers the UAC prompt).
pub fn relaunch_elevated() -> Result<(), String> {
    relaunch_elevated_with_args("")
}

/// Same as [`relaunch_elevated`], but passes a raw command line to the
/// elevated copy. Arguments are re-quoted by the caller.
pub fn relaunch_elevated_with_args(args: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot resolve own path: {}", e))?;
    let dir = exe
        .parent()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| "cannot resolve own directory".to_string())?;

    let file: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let operation = wide("runas");
    let directory: Vec<u16> = dir
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let params: Vec<u16> = if args.is_empty() {
        Vec::new()
    } else {
        wide(args)
    };
    let params_ptr = if params.is_empty() {
        std::ptr::null()
    } else {
        params.as_ptr()
    };

    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            params_ptr,
            directory.as_ptr(),
            SW_SHOWNORMAL,
        )
    };

    // ShellExecute returns a value > 32 on success.
    if result as isize > 32 {
        Ok(())
    } else {
        Err(format!(
            "UAC elevation was refused or failed (code {})",
            result as isize
        ))
    }
}
