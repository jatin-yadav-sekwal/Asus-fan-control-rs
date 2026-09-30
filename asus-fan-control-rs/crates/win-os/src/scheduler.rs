use std::path::Path;
use std::process::Command;
use tracing::{debug, info};
use crate::error::OsError;

const TASK_NAME: &str = "AsusFanControlAutoStart";

pub struct StartupManager;

impl StartupManager {
    /// Registers the application in Windows Task Scheduler to run at user logon with HighestAvailable privileges.
    /// This bypasses the UAC prompt on boot completely.
    pub fn enable_autostart(exe_path: &Path, arguments: &str) -> Result<(), OsError> {
        let task_run_cmd = if arguments.is_empty() {
            format!("\"{}\"", exe_path.display())
        } else {
            format!("\"{}\" {}", exe_path.display(), arguments)
        };

        info!("Registering Task Scheduler entry '{}' for: {}", TASK_NAME, task_run_cmd);

        let output = Command::new("schtasks.exe")
            .args([
                "/Create",
                "/TN", TASK_NAME,
                "/TR", &task_run_cmd,
                "/SC", "ONLOGON",
                "/RL", "HIGHEST",
                "/F", // Force overwrite if already present
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(OsError::TaskSchedulerFailed {
                exit_code: output.status.code().unwrap_or(-1),
                stderr,
            });
        }

        info!("Successfully registered Task Scheduler startup task.");
        Ok(())
    }

    /// Removes the scheduled startup task.
    pub fn disable_autostart() -> Result<(), OsError> {
        info!("Removing Task Scheduler entry '{}'", TASK_NAME);

        let output = Command::new("schtasks.exe")
            .args([
                "/Delete",
                "/TN", TASK_NAME,
                "/F",
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            debug!("Task Scheduler delete output: {}", stderr);
        }

        Ok(())
    }

    /// Checks if the Task Scheduler entry currently exists.
    pub fn is_autostart_enabled() -> bool {
        let output = Command::new("schtasks.exe")
            .args([
                "/Query",
                "/TN", TASK_NAME,
            ])
            .output();

        match output {
            Ok(out) => out.status.success(),
            Err(_) => false,
        }
    }
}
