use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DriverError {
    #[error("AsusWinIO64.dll could not be found in searched locations: {0:?}")]
    DllNotFound(Vec<PathBuf>),

    #[error("Failed to load AsusWinIO64.dll: {0}")]
    LoadLibraryFailed(#[from] libloading::Error),

    #[error("Missing native symbol '{symbol}' in AsusWinIO64.dll")]
    SymbolNotFound {
        symbol: &'static str,
        source: libloading::Error,
    },

    #[error("Invalid fan index {index}; system reported {max_fans} fans (valid indices: 0..{max_fans})")]
    InvalidFanIndex { index: u8, max_fans: u8 },

    #[error("Invalid fan speed percentage: {percent} (must be between 0 and 100)")]
    InvalidPercentage { percent: u8 },

    #[error(
        "Not running as NT AUTHORITY\\SYSTEM: the \\\\.\\AsusSAIO device rejects non-SYSTEM \
         callers, so every HealthyTable_* call returns -1.\nLaunch the app with run-gui.bat \
         (PsExec -i -s) instead of starting the .exe directly."
    )]
    ElevationRequired,

    #[error("Driver initialization failed or returned invalid response")]
    InitializationFailed,

    #[error(
        "AsusWinIO64.dll loaded but the AsusSAIO kernel driver is not responding ({reason}).\n\
         Another fan-control app (AsusFanControlGUI) may be holding \\\\.\\AsusSAIO, or the \
         driver service is stuck. Close other fan tools and relaunch via run-gui.bat."
    )]
    DriverNotReady { reason: String },

    #[error(
        "AsusWinIO64.dll was found but every HealthyTable_* export failed (fan count={fan_count}, \
         rpm={rpm}). Either the process is not SYSTEM or the driver service could not start."
    )]
    HealthyTableUnresponsive { fan_count: i32, rpm: i32 },

    #[error(
        "HealthyTable_FanRPM failed for fan {fan_index} (raw={value}); the AsusSAIO driver \
         stopped responding mid-read"
    )]
    FanReadFailed { fan_index: u8, value: i32 },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
