//! Preflight repair for the `AsusSAIO` kernel driver service.
//!
//! `AsusWinIO64.dll` creates/starts this service itself, but a previous run that
//! was killed mid-flight can leave the service `Disabled` or stuck in
//! `Stop Pending`. In that state every `HealthyTable_*` export returns `-1`,
//! which the old code silently turned into "0 RPM". We normalise the service
//! before handing over to the DLL.

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
fn token_user_sid_bytes() -> Option<Vec<u8>> {
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
             non-SYSTEM callers, so HealthyTable_* will return -1. Launch via run-gui.bat.",
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

/// Hides the console window of child console processes (PsExec).
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
/// elevation type. Needed because PsExec cannot install PSEXESVC otherwise.
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

    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
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

/// Restarts the current executable under `NT AUTHORITY\SYSTEM` via PsExec,
/// with no visible console windows.
///
/// `\\.\AsusSAIO` rejects every other account, so this is the only way a
/// double-clicked EXE can reach the driver.
pub fn launch_as_system(extra_args: &[&str]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot resolve own path: {}", e))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "cannot resolve own directory".to_string())?
        .to_path_buf();

    let psexec = find_psexec(&exe).ok_or_else(|| {
        format!(
            "PsExec.exe was not found next to {} or on PATH.\n\n\
             It is required to run this app as NT AUTHORITY\\SYSTEM, which is the \
             only account the ASUS driver accepts. Copy PsExec.exe into the same \
             folder as the application.",
            exe.display()
        )
    })?;

    info!(
        "Relaunching as SYSTEM: {:?} -w {:?} -i -s -d {:?} {:?}",
        psexec, dir, exe, extra_args
    );

    let own_pid = std::process::id();
    let exe_name = exe
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("asus-fan-app.exe")
        .to_string();
    let before = process_ids_named(&exe_name);

    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new(&psexec);
    command
        .arg("-accepteula")
        .arg("-w")
        .arg(&dir)
        .arg("-i")
        .arg("-s")
        .arg("-d")
        .arg(&exe)
        .args(extra_args)
        .creation_flags(CREATE_NO_WINDOW);

    let mut child = command
        .spawn()
        .map_err(|e| format!("failed to start PsExec: {}", e))?;

    // PsExec -d reports a misleading exit code (it returned the child PID on
    // this machine), so success is determined by observing the relaunched
    // process rather than trusting the status alone.
    let status = child
        .wait()
        .map_err(|e| format!("failed waiting for PsExec: {}", e))?;

    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        let now = process_ids_named(&exe_name);
        if now.iter().any(|pid| *pid != own_pid && !before.contains(pid)) {
            info!(
                "SYSTEM instance started (PsExec exit code {:?})",
                status.code()
            );
            return Ok(());
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    Err(format!(
        "PsExec could not start the application as SYSTEM (PsExec exit code {}). \
         Make sure PsExec.exe is not blocked by antivirus and that you \
         accepted the UAC prompt.",
        status.code().unwrap_or(-1)
    ))
}

/// PIDs of running processes with the given image name.
fn process_ids_named(name: &str) -> Vec<u32> {
    let output = match std::process::Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let cols: Vec<&str> = line.split("\",\"").collect();
            if cols.len() < 2 {
                return None;
            }
            if !name.eq_ignore_ascii_case(cols[0].trim_matches('"')) {
                return None;
            }
            cols[1].trim_matches('"').parse::<u32>().ok()
        })
        .collect()
}

fn find_psexec(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = exe.parent()?;
    let candidates: [Option<std::path::PathBuf>; 2] = [
        Some(dir.join("PsExec.exe")),
        dir.parent().map(|p| p.join("PsExec.exe")),
    ];
    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            return Some(candidate);
        }
    }

    if let Some(paths) = std::env::var_os("PATH") {
        for entry in std::env::split_paths(&paths) {
            let candidate = entry.join("PsExec.exe");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}
