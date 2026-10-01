//! The SYSTEM-side helper.
//!
//! Started by a one-shot Task Scheduler job running as `NT AUTHORITY\SYSTEM`,
//! which is the only account `\\.\AsusSAIO` answers to. It owns the driver for
//! as long as the GUI is connected and does nothing else — no window, no
//! rendering, session 0 is irrelevant because it never paints.
//!
//! Failure model: if the GUI goes away for *any* reason (closed, crashed,
//! killed) the pipe breaks, the serve loop exits, and the fans are returned to
//! BIOS control before this process terminates. The last commanded duty is
//! never left latched.

use std::time::Duration;

use tracing::{error, info, warn};

use crate::ipc;
use crate::task;
use crate::SafeAsusDriver;

/// How long the helper waits for the GUI to open the pipe before giving up.
///
/// A leaked task (app crashed before it could clean up) must not leave a
/// resident SYSTEM process behind.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);

/// Runs the helper until the client disconnects or asks for shutdown.
/// Returns the process exit code.
pub fn run(pipe_name: &str) -> i32 {
    info!("SYSTEM helper starting on {}", pipe_name);

    let handle = match ipc::create_server(pipe_name, CONNECT_TIMEOUT) {
        Ok(h) => h,
        Err(e) => {
            error!("Helper could not establish the control pipe: {}", e);
            task::stop_system_task();
            return 1;
        }
    };
    let server = ipc::PipeServer::new(handle);

    // Read the handshake *before* touching hardware, so an orphaned helper
    // never opens the EC, and a driver failure reaches the client as a proper
    // error response instead of a mysteriously broken pipe.
    let first = match server.read_request() {
        Ok(r) => r,
        Err(e) => {
            error!("Helper never received a handshake: {}", e);
            task::stop_system_task();
            return 1;
        }
    };

    let driver = match SafeAsusDriver::new() {
        Ok(d) => d,
        Err(e) => {
            error!("Helper could not open the ASUS driver: {}", e);
            let _ = server.write_err(&e.to_string());
            task::stop_system_task();
            return 1;
        }
    };
    info!(
        "Helper owns \\\\.\\AsusSAIO: {} fan(s), token SID {}",
        driver.lock().fan_count(),
        crate::service::token_sid_string()
    );

    if !serve_one(&server, &driver, &first) {
        return finish(&driver);
    }

    loop {
        let request = match server.read_request() {
            Ok(r) => r,
            Err(e) => {
                info!("GUI disconnected ({}); releasing hardware", e);
                break;
            }
        };
        if request.is_empty() {
            continue;
        }
        if !serve_one(&server, &driver, &request) {
            break;
        }
    }

    finish(&driver)
}

/// Returns `true` to keep serving, `false` on an orderly shutdown request or a
/// dead pipe.
fn serve_one(server: &ipc::PipeServer, driver: &SafeAsusDriver, request: &[u8]) -> bool {
    match server.handle_request(request, driver) {
        Ok(true) => true,
        Ok(false) => {
            info!("GUI requested helper shutdown");
            false
        }
        Err(e) => {
            warn!("Failed to answer a request: {}", e);
            false
        }
    }
}

/// Failsafe: hand control back to the BIOS whatever the disconnect reason,
/// then disappear along with the scheduled task.
fn finish(driver: &SafeAsusDriver) -> i32 {
    info!("Returning all fans to BIOS control");
    match driver.lock().reset_all_to_bios() {
        Ok(()) => info!("Fans restored to BIOS control"),
        Err(e) => warn!("Could not restore BIOS control: {}", e),
    }
    task::stop_system_task();
    0
}
