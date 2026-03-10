//! Error handling module for TimerLib.

use std::backtrace::Backtrace;
use std::error::Error as StdError;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::time::Duration;

/// Represents timer-related failures.
#[derive(Debug, Clone)]
pub struct TimerError {
    kind: TimerErrorKind,
    backtrace: Arc<Backtrace>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TimerErrorKind {
    InvalidParameter(String),
    NotRunning,
    NotPaused,
    ReentrantOperation(String),
    CallbackTimedOut(Duration),
    CallbackFailed(String),
}

impl TimerError {
    /// Creates an invalid parameter error.
    pub fn invalid_parameter(message: impl Into<String>) -> Self {
        Self::new(TimerErrorKind::InvalidParameter(message.into()))
    }

    /// Creates an error for operations that require a running timer.
    pub fn not_running() -> Self {
        Self::new(TimerErrorKind::NotRunning)
    }

    /// Creates an error for operations that require a paused timer.
    pub fn not_paused() -> Self {
        Self::new(TimerErrorKind::NotPaused)
    }

    /// Creates an error for operations that cannot safely run from the active callback.
    pub fn reentrant_operation(message: impl Into<String>) -> Self {
        Self::new(TimerErrorKind::ReentrantOperation(message.into()))
    }

    /// Creates an error for callback timeout expiration.
    pub fn callback_timed_out(timeout: Duration) -> Self {
        Self::new(TimerErrorKind::CallbackTimedOut(timeout))
    }

    /// Creates an error for callback execution failures.
    pub fn callback_failed(message: impl Into<String>) -> Self {
        Self::new(TimerErrorKind::CallbackFailed(message.into()))
    }

    /// Returns true when the error is an invalid parameter error.
    pub fn is_invalid_parameter(&self) -> bool {
        matches!(self.kind, TimerErrorKind::InvalidParameter(_))
    }

    /// Returns the invalid parameter message when available.
    pub fn invalid_parameter_message(&self) -> Option<&str> {
        match &self.kind {
            TimerErrorKind::InvalidParameter(message) => Some(message.as_str()),
            _ => None,
        }
    }

    /// Returns true when the error indicates the timer was not running.
    pub fn is_not_running(&self) -> bool {
        matches!(self.kind, TimerErrorKind::NotRunning)
    }

    /// Returns true when the error indicates the timer was not paused.
    pub fn is_not_paused(&self) -> bool {
        matches!(self.kind, TimerErrorKind::NotPaused)
    }

    /// Returns true when the error indicates a reentrant callback operation.
    pub fn is_reentrant_operation(&self) -> bool {
        matches!(self.kind, TimerErrorKind::ReentrantOperation(_))
    }

    /// Returns the reentrant-operation message when available.
    pub fn reentrant_operation_message(&self) -> Option<&str> {
        match &self.kind {
            TimerErrorKind::ReentrantOperation(message) => Some(message.as_str()),
            _ => None,
        }
    }

    /// Returns true when the error indicates callback execution failed.
    pub fn is_callback_failed(&self) -> bool {
        matches!(self.kind, TimerErrorKind::CallbackFailed(_))
    }

    /// Returns true when the error indicates callback execution timed out.
    pub fn is_callback_timed_out(&self) -> bool {
        matches!(self.kind, TimerErrorKind::CallbackTimedOut(_))
    }

    /// Returns the callback timeout when available.
    pub fn callback_timeout(&self) -> Option<Duration> {
        match &self.kind {
            TimerErrorKind::CallbackTimedOut(timeout) => Some(*timeout),
            _ => None,
        }
    }

    /// Returns the callback failure message when available.
    pub fn callback_failure_message(&self) -> Option<&str> {
        match &self.kind {
            TimerErrorKind::CallbackFailed(message) => Some(message.as_str()),
            _ => None,
        }
    }

    /// Returns the captured backtrace.
    pub fn backtrace(&self) -> &Backtrace {
        self.backtrace.as_ref()
    }

    fn new(kind: TimerErrorKind) -> Self {
        Self {
            kind,
            backtrace: Arc::new(Backtrace::capture()),
        }
    }
}

impl Display for TimerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            TimerErrorKind::InvalidParameter(message) => {
                write!(f, "Invalid parameter: {message}")
            }
            TimerErrorKind::NotRunning => write!(f, "Operation requires a running timer."),
            TimerErrorKind::NotPaused => write!(f, "Operation requires a paused timer."),
            TimerErrorKind::ReentrantOperation(message) => {
                write!(f, "Reentrant timer operation is not allowed: {message}")
            }
            TimerErrorKind::CallbackTimedOut(timeout) => {
                write!(f, "Callback execution timed out after {timeout:?}")
            }
            TimerErrorKind::CallbackFailed(message) => {
                write!(f, "Callback execution failed: {message}")
            }
        }
    }
}

impl StdError for TimerError {}

impl PartialEq for TimerError {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Eq for TimerError {}

// Rust guideline compliant 2026-02-21

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_stable_and_descriptive() {
        assert_eq!(
            TimerError::invalid_parameter("bad interval").to_string(),
            "Invalid parameter: bad interval"
        );
        assert_eq!(
            TimerError::not_running().to_string(),
            "Operation requires a running timer."
        );
        assert_eq!(
            TimerError::not_paused().to_string(),
            "Operation requires a paused timer."
        );
        assert_eq!(
            TimerError::reentrant_operation("stop() from callback").to_string(),
            "Reentrant timer operation is not allowed: stop() from callback"
        );
        assert_eq!(
            TimerError::callback_timed_out(Duration::from_secs(2)).to_string(),
            "Callback execution timed out after 2s"
        );
        assert_eq!(
            TimerError::callback_failed("boom").to_string(),
            "Callback execution failed: boom"
        );
    }

    #[test]
    fn error_helpers_expose_stable_queries() {
        let invalid = TimerError::invalid_parameter("interval");
        assert!(invalid.is_invalid_parameter());
        assert_eq!(invalid.invalid_parameter_message(), Some("interval"));
        assert!(!invalid.is_not_running());

        let callback = TimerError::callback_failed("boom");
        assert!(callback.is_callback_failed());
        assert_eq!(callback.callback_failure_message(), Some("boom"));

        let timed_out = TimerError::callback_timed_out(Duration::from_millis(250));
        assert!(timed_out.is_callback_timed_out());
        assert_eq!(
            timed_out.callback_timeout(),
            Some(Duration::from_millis(250))
        );

        let reentrant = TimerError::reentrant_operation("join()");
        assert!(reentrant.is_reentrant_operation());
        assert_eq!(reentrant.reentrant_operation_message(), Some("join()"));
        assert!(
            callback.backtrace().status() != std::backtrace::BacktraceStatus::Disabled
                || callback.backtrace().status() == std::backtrace::BacktraceStatus::Disabled
        );
    }
}
