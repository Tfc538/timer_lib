use async_trait::async_trait;
use futures;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time;

#[cfg(feature = "logging")]
use log::{debug, error};

use crate::errors::TimerError;

/// Represents the state of a timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerState {
    Running,
    Paused,
    Stopped,
}

/// Statistics for a timer.
#[derive(Debug, Clone, Default)]
pub struct TimerStatistics {
    /// Number of times the callback has been executed.
    pub execution_count: usize,
    /// Total elapsed time since the timer started.
    pub elapsed_time: Duration,
}

/// A trait for timer callbacks.
#[async_trait]
pub trait TimerCallback: Send + Sync {
    /// The function to execute when the timer triggers.
    async fn execute(&self) -> Result<(), TimerError>;
}

#[derive(Clone)]
/// Timer struct for managing one-time and recurring tasks.
pub struct Timer {
    state: Arc<Mutex<TimerState>>,
    handle: Option<Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>>,
    interval: Duration,
    expiration_count: Option<usize>,
    statistics: Arc<Mutex<TimerStatistics>>,
    pause_notify: Arc<Notify>,
}

impl Timer {
    /// Creates a new timer.
    pub fn new() -> Self {
        Timer {
            state: Arc::new(Mutex::new(TimerState::Stopped)),
            handle: None,
            interval: Duration::from_secs(0),
            expiration_count: None,
            statistics: Arc::new(Mutex::new(TimerStatistics::default())),
            pause_notify: Arc::new(Notify::new()),
        }
    }

    /// Starts a one-time timer.
    pub async fn start_once<F>(&mut self, delay: Duration, callback: F) -> Result<(), TimerError>
    where
        F: TimerCallback + 'static,
    {
        self.start_internal(delay, callback, false, None).await
    }

    /// Starts a recurring timer with an optional expiration count.
    pub async fn start_recurring<F>(
        &mut self,
        interval: Duration,
        callback: F,
        expiration_count: Option<usize>,
    ) -> Result<(), TimerError>
    where
        F: TimerCallback + 'static,
    {
        self.start_internal(interval, callback, true, expiration_count)
            .await
    }

    /// Pauses a running timer.
    pub async fn pause(&self) -> Result<(), TimerError> {
        let mut state = self.state.lock().await;
        if *state == TimerState::Running {
            *state = TimerState::Paused;
            #[cfg(feature = "logging")]
            debug!("Timer paused.");
            Ok(())
        } else {
            Err(TimerError::TimerStopped)
        }
    }

    /// Resumes a paused timer.
    pub async fn resume(&self) -> Result<(), TimerError> {
        let mut state = self.state.lock().await;
        if *state == TimerState::Paused {
            *state = TimerState::Running;
            self.pause_notify.notify_one();
            #[cfg(feature = "logging")]
            debug!("Timer resumed.");
            Ok(())
        } else {
            Err(TimerError::InvalidParameter("Timer is not paused.".into()))
        }
    }

    /// Stops the timer.
    pub async fn stop(&mut self) -> Result<(), TimerError> {
        let mut state = self.state.lock().await;
        if *state != TimerState::Stopped {
            *state = TimerState::Stopped;
            if let Some(handle) = self.handle.take() {
                drop(state); // Release the lock before awaiting
                #[cfg(feature = "logging")]
                debug!("Stopping timer.");
                if let Some(handle) = handle.lock().await.take() {
                    handle.abort();
                }
            }
            Ok(())
        } else {
            Err(TimerError::TimerStopped)
        }
    }

    /// Adjusts the interval of a running timer.
    pub fn adjust_interval(&mut self, new_interval: Duration) -> Result<(), TimerError> {
        if new_interval.as_nanos() == 0 {
            return Err(TimerError::InvalidParameter(
                "Interval must be greater than zero.".into(),
            ));
        }
        self.interval = new_interval;
        #[cfg(feature = "logging")]
        debug!("Timer interval adjusted.");
        Ok(())
    }

    /// Gets the timer's statistics.
    pub async fn get_statistics(&self) -> TimerStatistics {
        self.statistics.lock().await.clone()
    }

    /// Gets the current state of the timer.
    pub async fn get_state(&self) -> TimerState {
        *self.state.lock().await
    }

    /// Internal method to start a timer.
    /// Internal method to start a timer.
    async fn start_internal<F>(
        &mut self,
        interval: Duration,
        callback: F,
        recurring: bool,
        expiration_count: Option<usize>,
    ) -> Result<(), TimerError>
    where
        F: TimerCallback + 'static,
    {
        if interval.as_nanos() == 0 {
            return Err(TimerError::InvalidParameter(
                "Interval must be greater than zero.".into(),
            ));
        }

        if let Err(e) = self.stop().await {
            #[cfg(feature = "logging")]
            error!("Failed to stop existing timer: {}", e);
        }

        // Set the timer state to Running
        {
            let mut state_lock = self.state.lock().await;
            *state_lock = TimerState::Running;
        }

        // Clone variables for the async move closure
        let state = Arc::clone(&self.state);
        let statistics = Arc::clone(&self.statistics);
        let pause_notify = Arc::clone(&self.pause_notify);
        let interval = interval;
        let expiration_count = expiration_count;
        let callback = Arc::new(callback); // Wrap the callback in Arc

        #[cfg(feature = "logging")]
        debug!("Starting timer.");

        self.handle = Some(Arc::new(Mutex::new(Some(tokio::spawn(async move {
            let mut tick_count = 0;
            let start_time = Instant::now();

            loop {
                // Check the timer state
                let current_state = {
                    let state_lock = state.lock().await;
                    *state_lock
                };

                if current_state == TimerState::Stopped {
                    break;
                } else if current_state == TimerState::Paused {
                    pause_notify.notified().await;
                    continue;
                }

                // Wait for the interval
                time::sleep(interval).await;

                // Execute the callback
                if let Err(e) = callback.execute().await {
                    #[cfg(feature = "logging")]
                    error!("Callback execution error: {}", e);
                }

                // Update statistics
                {
                    let mut stats = statistics.lock().await;
                    stats.execution_count += 1;
                    stats.elapsed_time = start_time.elapsed();
                }
                tick_count += 1;

                // Check expiration count
                if let Some(max_ticks) = expiration_count {
                    if tick_count >= max_ticks {
                        #[cfg(feature = "logging")]
                        debug!("Timer reached expiration count.");
                        break;
                    }
                }

                if !recurring {
                    break;
                }
            }

            #[cfg(feature = "logging")]
            debug!("Timer stopped.");
        })))));

        Ok(())
    }
}

unsafe impl Send for Timer {}
unsafe impl Sync for Timer {}
