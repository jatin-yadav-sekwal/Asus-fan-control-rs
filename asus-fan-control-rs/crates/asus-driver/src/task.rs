//! Native `NT AUTHORITY\SYSTEM` bootstrap via Windows Task Scheduler.
//!
//! `\\.\AsusSAIO` only answers to SYSTEM, so something has to launch a process
//! with that token. PsExec does it, but its licence forbids redistribution
//! (*"No part of PsTools may be redistributed in any way"*), so the app instead
//! registers a one-shot scheduled task running as SYSTEM and starts it.
//!
//! Task Scheduler ships with every copy of Windows — nothing is bundled, no
//! licence is implicated, and no console window is shown because `schtasks` is
//! spawned with `CREATE_NO_WINDOW`.

use std::ffi::c_void;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use tracing::{info, warn};

/// Task name used for the SYSTEM helper. Deliberately descriptive: AV products
/// treat obfuscated task names as a persistence signal.
pub const HELPER_TASK_NAME: &str = "AsusFanControlSystemHelper";

/// Hides the console window of `schtasks.exe`.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Quote an argument if it contains characters `schtasks` would split on.
fn quote_arg(arg: &str) -> String {
    if arg.contains(' ') || arg.contains('\t') {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

fn schtasks(args: &[&str]) -> io::Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    std::process::Command::new("schtasks.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

/// Formats a time `minutes` from now as `HH:mm` (24-hour, which `schtasks
/// /ST` accepts regardless of the regional short-time format).
fn time_in(minutes: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
        + minutes * 60;
    let secs_of_day = now.rem_euclid(86_400);
    format!("{:02}:{:02}", secs_of_day / 3600, (secs_of_day % 3600) / 60)
}

/// Registers a one-shot task running `exe args...` as `NT AUTHORITY\SYSTEM`
/// and starts it.
///
/// The task is scheduled a few minutes out so it never fires on its own; it is
/// started explicitly with `/Run` and removed with [`stop_system_task`]. If the
/// app dies before cleaning up, the helper itself gives up waiting for a client
/// and exits, so a leaked task cannot leave a resident process.
pub fn start_system_task(exe: &Path, args: &[&str]) -> Result<(), String> {
    let mut cmdline = format!("\"{}\"", exe.display());
    for arg in args {
        cmdline.push(' ');
        cmdline.push_str(&quote_arg(arg));
    }

    // Remove any stale entry left behind by a previous run that crashed.
    let _ = schtasks(&["/Delete", "/TN", HELPER_TASK_NAME, "/F"]);

    let create = schtasks(&[
        "/Create",
        "/TN",
        HELPER_TASK_NAME,
        "/TR",
        &cmdline,
        "/SC",
        "ONCE",
        "/ST",
        &time_in(10),
        "/RU",
        "SYSTEM",
        "/RL",
        "HIGHEST",
        "/F",
    ])
    .map_err(|e| format!("could not run schtasks.exe: {}", e))?;

    if !create.status.success() {
        return Err(format!(
            "schtasks /Create failed (exit {}): {}",
            create.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&create.stderr).trim()
        ));
    }

    let run = schtasks(&["/Run", "/TN", HELPER_TASK_NAME])
        .map_err(|e| format!("could not run schtasks.exe: {}", e))?;
    if !run.status.success() {
        return Err(format!(
            "schtasks /Run failed (exit {}): {}",
            run.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&run.stderr).trim()
        ));
    }

    info!("SYSTEM helper task '{}' started: {}", HELPER_TASK_NAME, cmdline);
    Ok(())
}

/// Deletes the helper task. Best-effort: a missing task is not an error, and
/// the caller must not fail its own shutdown because of this.
pub fn stop_system_task() {
    match schtasks(&["/Delete", "/TN", HELPER_TASK_NAME, "/F"]) {
        Ok(out) if out.status.success() => {
            info!("SYSTEM helper task '{}' removed", HELPER_TASK_NAME)
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
            // 1 = "cannot find the file specified" — already gone.
            if out.status.code() != Some(1) {
                warn!("schtasks /Delete returned {}: {}", out.status.code().unwrap_or(-1), stderr);
            }
        }
        Err(e) => warn!("schtasks /Delete could not run: {}", e),
    }
}

/// Polls the task's state until it reports running, or the deadline passes.
///
/// Only used for diagnostics — the real readiness signal is the pipe handshake
/// (`PipeClient::connect`), which already has its own timeout.
pub fn wait_for_task_running(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(out) = schtasks(&["/Query", "/TN", HELPER_TASK_NAME, "/FO", "CSV", "/NH"]) {
            let text = String::from_utf8_lossy(&out.stdout).to_uppercase();
            if text.contains("RUNNING") {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// First argument of the helper process: switch it into headless SYSTEM mode.
pub const HELPER_ARG: &str = "--sys-helper";
/// Prefix of the second argument: the named pipe to serve.
pub const PIPE_ARG_PREFIX: &str = "--pipe=";

/// Builds an unguessable pipe name.
///
/// The pipe DACL already restricts access to SYSTEM plus the current user, so
/// randomness is defence in depth rather than the only barrier — but a GUID
/// name also keeps two independent sessions from colliding if the mutex guard
/// is ever bypassed.
pub fn make_pipe_name() -> String {
    #[repr(C)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[link(name = "ole32")]
    extern "system" {
        fn CoCreateGuid(g: *mut Guid) -> i32;
    }

    let mut g = Guid {
        data1: 0,
        data2: 0,
        data3: 0,
        data4: [0; 8],
    };
    let ok = unsafe { CoCreateGuid(&mut g) };
    if ok == 0 {
        let d4: String = g.data4.iter().map(|b| format!("{:02x}", b)).collect();
        format!(
            "\\\\.\\pipe\\AsusFanControl-{:08x}-{:04x}-{:04x}-{}",
            g.data1, g.data2, g.data3, d4
        )
    } else {
        format!(
            "\\\\.\\pipe\\AsusFanControl-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        )
    }
}

// ---------------------------------------------------------------------------
// Single-instance guard
// ---------------------------------------------------------------------------

/// Machine-wide mutex so a second GUI cannot delete the running helper's task
/// and start a competing one.
const INSTANCE_MUTEX: &str = "Global\\AsusFanControlSingleInstance";
const ERROR_ALREADY_EXISTS: u32 = 183;

static INSTANCE_HANDLE: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);

#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(
        mutex_attributes: *mut c_void,
        initial_owner: i32,
        name: *const u16,
    ) -> *mut c_void;
    fn CloseHandle(object: *mut c_void) -> i32;
    #[link_name = "GetLastError"]
    fn last_error() -> u32;
}

/// Takes the single-instance lock. Returns `false` when another copy of the
/// application is already running.
///
/// The lock is held for as long as the process lives; the OS releases it
/// automatically if the process dies, so a crash never wedges the next launch.
pub fn acquire_single_instance() -> bool {
    let name = wide(INSTANCE_MUTEX);
    let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 1, name.as_ptr()) };
    if handle.is_null() {
        warn!("CreateMutexW failed (error {}); skipping single-instance guard", unsafe {
            last_error()
        });
        return true;
    }

    // ERROR_ALREADY_EXISTS means the mutex existed before this call, i.e.
    // another instance created it (with initial_owner we do not own it here).
    let already = unsafe { last_error() } == ERROR_ALREADY_EXISTS;
    if already {
        unsafe { CloseHandle(handle) };
        return false;
    }

    if let Ok(mut slot) = INSTANCE_HANDLE.lock() {
        *slot = Some(handle as usize);
    }
    true
}

/// Releases the single-instance lock. Safe to call more than once.
pub fn release_single_instance() {
    if let Ok(mut slot) = INSTANCE_HANDLE.lock() {
        if let Some(raw) = slot.take() {
            unsafe { CloseHandle(raw as *mut c_void) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_arguments_with_spaces() {
        assert_eq!(quote_arg("plain"), "plain");
        assert_eq!(quote_arg("with space"), "\"with space\"");
    }

    #[test]
    fn scheduled_time_is_well_formed() {
        let t = time_in(10);
        assert_eq!(t.len(), 5);
        assert_eq!(t.as_bytes()[2], b':');
        let (h, m) = t.split_at(2);
        assert!(h.parse::<u32>().unwrap() < 24);
        assert!(m[1..].parse::<u32>().unwrap() < 60);
    }
}
