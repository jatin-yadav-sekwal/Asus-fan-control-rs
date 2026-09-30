use thiserror::Error;

#[derive(Error, Debug)]
pub enum OsError {
    #[error("Task Scheduler command failed with code {exit_code}: {stderr}")]
    TaskSchedulerFailed { exit_code: i32, stderr: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
