//! # TimerLib
//! A robust and feature-rich Rust timer library for one-time and recurring timers.

pub mod errors;
pub mod manager;
pub mod timer;

pub use errors::TimerError;
pub use manager::TimerManager;
pub use timer::{Timer, TimerCallback, TimerState, TimerStatistics};
