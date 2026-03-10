//! Tokio-based timers for one-shot and recurring async work.
//!
//! `timer-lib` provides a handle-first API for scheduling async callbacks and
//! observing their lifecycle.
//!
//! # Overview
//!
//! - [`Timer`] runs one-shot or recurring work.
//! - [`TimerBuilder`] reduces setup boilerplate for common configurations.
//! - [`TimerRegistry`] tracks timers by ID and supports bulk operations.
//! - [`TimerEvents`] exposes broadcast lifecycle events.
//! - [`TimerCompletion`] exposes lossless completed-run delivery.
//!
//! # Examples
//!
//! Start a one-shot timer and wait for it to finish:
//!
//! ```rust
//! use std::time::Duration;
//! use timer_lib::{Timer, TimerError, TimerFinishReason};
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread()
//! #     .enable_all()
//! #     .build()
//! #     .unwrap();
//! # runtime.block_on(async {
//! let timer = Timer::new();
//! timer
//!     .start_once(Duration::from_millis(10), || async { Ok::<(), TimerError>(()) })
//!     .await
//!     .unwrap();
//!
//! let outcome = timer.join().await.unwrap();
//! assert_eq!(outcome.reason, TimerFinishReason::Completed);
//! # });
//! ```
//!
//! Build a recurring timer:
//!
//! ```rust
//! use std::time::Duration;
//! use timer_lib::{RecurringSchedule, Timer, TimerError, TimerFinishReason};
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread()
//! #     .enable_all()
//! #     .build()
//! #     .unwrap();
//! # runtime.block_on(async {
//! let timer = Timer::recurring(RecurringSchedule::new(Duration::from_millis(10)).with_expiration_count(2))
//!     .start(|| async { Ok::<(), TimerError>(()) })
//!     .await
//!     .unwrap();
//!
//! let outcome = timer.join().await.unwrap();
//! assert_eq!(outcome.reason, TimerFinishReason::Completed);
//! assert_eq!(outcome.statistics.execution_count, 2);
//! # });
//! ```
//!
//! Observe completion without relying on lossy broadcast delivery:
//!
//! ```rust
//! use std::time::Duration;
//! use timer_lib::{Timer, TimerError};
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread()
//! #     .enable_all()
//! #     .build()
//! #     .unwrap();
//! # runtime.block_on(async {
//! let timer = Timer::new();
//! let mut completion = timer.completion();
//! let run_id = timer
//!     .start_once(Duration::from_millis(10), || async { Ok::<(), TimerError>(()) })
//!     .await
//!     .unwrap();
//!
//! let outcome = completion.wait_for_run(run_id).await.unwrap();
//! assert_eq!(outcome.run_id, run_id);
//! # });
//! ```
//!
//! # Runtime
//!
//! This crate currently targets Tokio.
//!
//! # Errors
//!
//! Public operations return [`TimerError`] for invalid configuration or invalid
//! lifecycle transitions.

pub mod errors;
pub mod registry;
pub mod timer;

pub use errors::TimerError;
pub use registry::TimerRegistry;
#[deprecated(note = "Use TimerRegistry instead.")]
pub type TimerManager = TimerRegistry;
pub use registry::RegisteredTimer;
#[cfg(feature = "test-util")]
pub use timer::MockRuntime;
pub use timer::{
    RecurringCadence, RecurringSchedule, RetryBackoff, RetryPolicy, Timer, TimerBuilder,
    TimerCallback, TimerCompletion, TimerEvent, TimerEvents, TimerFinishReason, TimerMetadata,
    TimerOutcome, TimerSnapshot, TimerState, TimerStatistics,
};

// Rust guideline compliant 2026-02-21
