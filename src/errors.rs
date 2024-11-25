//! Error handling module for TimerLib.

use thiserror::Error;

/// Custom error type for Timer operations.
#[derive(Error, Debug)]
pub enum TimerError {
    /// Invalid parameter provided.
    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),

    /// Operation attempted on a stopped timer.
    #[error("Operation attempted on a stopped timer.")]
    TimerStopped,

    /// Callback execution failed.
    #[error("Callback execution failed: {0}")]
    CallbackError(String),
}
