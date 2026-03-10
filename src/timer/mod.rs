use async_trait::async_trait;
use std::future::Future;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, watch, Mutex};
use tokio::task::JoinHandle;

#[cfg(feature = "logging")]
use log::debug;

use crate::errors::TimerError;

mod driver;
mod runtime;

#[cfg(test)]
mod tests;

const TIMER_EVENT_BUFFER: usize = 64;

/// Represents the state of a timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerState {
    Running,
    Paused,
    Stopped,
}

/// Indicates how a timer run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerFinishReason {
    Completed,
    Stopped,
    Cancelled,
    Replaced,
}

/// Statistics for a timer run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimerStatistics {
    /// Number of callback executions attempted during the current run.
    pub execution_count: usize,
    /// Number of successful callback executions.
    pub successful_executions: usize,
    /// Number of failed callback executions.
    pub failed_executions: usize,
    /// Total elapsed time since the current run started.
    pub elapsed_time: Duration,
    /// The most recent callback error observed in the current run.
    pub last_error: Option<TimerError>,
}

/// Describes the result of a completed timer run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimerOutcome {
    /// The completed run identifier.
    pub run_id: u64,
    /// The reason the run ended.
    pub reason: TimerFinishReason,
    /// Final run statistics.
    pub statistics: TimerStatistics,
}

/// Configures retry behavior for failed callback executions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    max_retries: usize,
}

impl RetryPolicy {
    /// Creates a retry policy with the provided retry limit.
    pub fn new(max_retries: usize) -> Self {
        Self { max_retries }
    }

    /// Returns the maximum number of retry attempts after the first failure.
    pub fn max_retries(self) -> usize {
        self.max_retries
    }
}

/// Event stream item produced by a timer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimerEvent {
    Started {
        run_id: u64,
        interval: Duration,
        recurring: bool,
        expiration_count: Option<usize>,
    },
    Paused {
        run_id: u64,
    },
    Resumed {
        run_id: u64,
    },
    IntervalAdjusted {
        run_id: u64,
        interval: Duration,
    },
    Tick {
        run_id: u64,
        statistics: TimerStatistics,
    },
    CallbackFailed {
        run_id: u64,
        error: TimerError,
        statistics: TimerStatistics,
    },
    Finished(TimerOutcome),
}

/// Subscription handle for timer events.
pub struct TimerEvents {
    receiver: broadcast::Receiver<TimerEvent>,
}

impl TimerEvents {
    /// Attempts to receive the next timer event without waiting.
    pub fn try_recv(&mut self) -> Option<TimerEvent> {
        loop {
            match self.receiver.try_recv() {
                Ok(event) => return Some(event),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(broadcast::error::TryRecvError::Empty)
                | Err(broadcast::error::TryRecvError::Closed) => return None,
            }
        }
    }

    /// Waits for the next timer event.
    pub async fn recv(&mut self) -> Option<TimerEvent> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Waits for the next start event.
    pub async fn wait_started(&mut self) -> Option<TimerEvent> {
        loop {
            if let event @ TimerEvent::Started { .. } = self.recv().await? {
                return Some(event);
            }
        }
    }

    /// Waits for the next tick event.
    pub async fn wait_tick(&mut self) -> Option<TimerEvent> {
        loop {
            if let event @ TimerEvent::Tick { .. } = self.recv().await? {
                return Some(event);
            }
        }
    }

    /// Waits for the next paused event.
    pub async fn wait_paused(&mut self) -> Option<TimerEvent> {
        loop {
            if let event @ TimerEvent::Paused { .. } = self.recv().await? {
                return Some(event);
            }
        }
    }

    /// Waits for the next resumed event.
    pub async fn wait_resumed(&mut self) -> Option<TimerEvent> {
        loop {
            if let event @ TimerEvent::Resumed { .. } = self.recv().await? {
                return Some(event);
            }
        }
    }

    /// Waits for the next finished event.
    pub async fn wait_finished(&mut self) -> Option<TimerOutcome> {
        loop {
            if let TimerEvent::Finished(outcome) = self.recv().await? {
                return Some(outcome);
            }
        }
    }

    /// Waits for the next finished event with a stopped outcome.
    pub async fn wait_stopped(&mut self) -> Option<TimerOutcome> {
        self.wait_finished_reason(TimerFinishReason::Stopped).await
    }

    /// Waits for the next finished event with a cancelled outcome.
    pub async fn wait_cancelled(&mut self) -> Option<TimerOutcome> {
        self.wait_finished_reason(TimerFinishReason::Cancelled)
            .await
    }

    async fn wait_finished_reason(&mut self, reason: TimerFinishReason) -> Option<TimerOutcome> {
        loop {
            let outcome = self.wait_finished().await?;
            if outcome.reason == reason {
                return Some(outcome);
            }
        }
    }
}

/// Lossless subscription handle for completed timer runs.
pub struct TimerCompletion {
    receiver: watch::Receiver<Option<TimerOutcome>>,
}

impl TimerCompletion {
    /// Returns the latest completed outcome, if one has been observed.
    pub fn latest(&self) -> Option<TimerOutcome> {
        self.receiver.borrow().clone()
    }

    /// Waits for the next unseen completed outcome.
    pub async fn wait(&mut self) -> Option<TimerOutcome> {
        loop {
            if let Some(outcome) = self.receiver.borrow_and_update().clone() {
                return Some(outcome);
            }

            if self.receiver.changed().await.is_err() {
                return self.receiver.borrow_and_update().clone();
            }
        }
    }

    /// Waits for a specific run to complete.
    pub async fn wait_for_run(&mut self, run_id: u64) -> Option<TimerOutcome> {
        loop {
            let outcome = self.wait().await?;
            if outcome.run_id == run_id {
                return Some(outcome);
            }
        }
    }
}

/// A trait for timer callbacks.
#[async_trait]
pub trait TimerCallback: Send + Sync {
    /// The function to execute when the timer triggers.
    async fn execute(&self) -> Result<(), TimerError>;
}

#[async_trait]
impl<F, Fut> TimerCallback for F
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Result<(), TimerError>> + Send,
{
    async fn execute(&self) -> Result<(), TimerError> {
        (self)().await
    }
}

pub(super) enum TimerCommand {
    Pause,
    Resume,
    Stop,
    Cancel,
    SetInterval(Duration),
}

pub(super) struct TimerInner {
    pub(super) state: Mutex<TimerState>,
    pub(super) handle: Mutex<Option<JoinHandle<()>>>,
    pub(super) command_tx: Mutex<Option<mpsc::UnboundedSender<TimerCommand>>>,
    pub(super) interval: Mutex<Duration>,
    pub(super) expiration_count: Mutex<Option<usize>>,
    pub(super) statistics: Mutex<TimerStatistics>,
    pub(super) last_outcome: Mutex<Option<TimerOutcome>>,
    pub(super) completion_tx: watch::Sender<Option<TimerOutcome>>,
    pub(super) event_tx: broadcast::Sender<TimerEvent>,
    pub(super) events_enabled: AtomicBool,
    pub(super) runtime: driver::RuntimeHandle,
    pub(super) next_run_id: AtomicU64,
    pub(super) active_run_id: AtomicU64,
}

#[derive(Debug, Clone, Copy)]
enum TimerKind {
    Once(Duration),
    Recurring {
        interval: Duration,
        initial_delay: Option<Duration>,
    },
}

/// Builds and starts a timer with less boilerplate.
pub struct TimerBuilder {
    kind: TimerKind,
    expiration_count: Option<usize>,
    callback_timeout: Option<Duration>,
    retry_policy: Option<RetryPolicy>,
    start_paused: bool,
    events_enabled: bool,
}

#[derive(Clone)]
/// Timer handle for managing one-time and recurring tasks.
pub struct Timer {
    inner: Arc<TimerInner>,
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

impl Timer {
    /// Creates a new timer.
    pub fn new() -> Self {
        Self::new_with_events(true)
    }

    /// Creates a new timer with broadcast events disabled.
    pub fn new_silent() -> Self {
        Self::new_with_events(false)
    }

    fn new_with_events(events_enabled: bool) -> Self {
        let (completion_tx, _completion_rx) = watch::channel(None);
        let (event_tx, _event_rx) = broadcast::channel(TIMER_EVENT_BUFFER);

        Self {
            inner: Arc::new(TimerInner {
                state: Mutex::new(TimerState::Stopped),
                handle: Mutex::new(None),
                command_tx: Mutex::new(None),
                interval: Mutex::new(Duration::ZERO),
                expiration_count: Mutex::new(None),
                statistics: Mutex::new(TimerStatistics::default()),
                last_outcome: Mutex::new(None),
                completion_tx,
                event_tx,
                events_enabled: AtomicBool::new(events_enabled),
                runtime: driver::RuntimeHandle,
                next_run_id: AtomicU64::new(1),
                active_run_id: AtomicU64::new(0),
            }),
        }
    }

    /// Creates a timer builder configured for a one-time run.
    pub fn once(delay: Duration) -> TimerBuilder {
        TimerBuilder::once(delay)
    }

    /// Creates a timer builder configured for a recurring run.
    pub fn recurring(interval: Duration) -> TimerBuilder {
        TimerBuilder::recurring(interval)
    }

    /// Subscribes to future timer events.
    pub fn subscribe(&self) -> TimerEvents {
        TimerEvents {
            receiver: self.inner.event_tx.subscribe(),
        }
    }

    /// Subscribes to completed runs without loss.
    pub fn completion(&self) -> TimerCompletion {
        TimerCompletion {
            receiver: self.inner.completion_tx.subscribe(),
        }
    }

    /// Starts a one-time timer.
    pub async fn start_once<F>(&self, delay: Duration, callback: F) -> Result<u64, TimerError>
    where
        F: TimerCallback + 'static,
    {
        self.start_internal(delay, None, None, None, callback, false, None, false)
            .await
    }

    /// Starts a one-time timer from an async closure.
    pub async fn start_once_fn<F, Fut>(
        &self,
        delay: Duration,
        callback: F,
    ) -> Result<u64, TimerError>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), TimerError>> + Send + 'static,
    {
        self.start_once(delay, callback).await
    }

    /// Starts a recurring timer with an optional expiration count.
    pub async fn start_recurring<F>(
        &self,
        interval: Duration,
        callback: F,
        expiration_count: Option<usize>,
    ) -> Result<u64, TimerError>
    where
        F: TimerCallback + 'static,
    {
        self.start_recurring_with_delay(interval, None, callback, expiration_count)
            .await
    }

    /// Starts a recurring timer from an async closure.
    pub async fn start_recurring_fn<F, Fut>(
        &self,
        interval: Duration,
        callback: F,
        expiration_count: Option<usize>,
    ) -> Result<u64, TimerError>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), TimerError>> + Send + 'static,
    {
        self.start_recurring(interval, callback, expiration_count)
            .await
    }

    /// Starts a recurring timer with an optional initial delay.
    pub async fn start_recurring_with_delay<F>(
        &self,
        interval: Duration,
        initial_delay: Option<Duration>,
        callback: F,
        expiration_count: Option<usize>,
    ) -> Result<u64, TimerError>
    where
        F: TimerCallback + 'static,
    {
        self.start_internal(
            interval,
            initial_delay,
            None,
            None,
            callback,
            true,
            expiration_count,
            false,
        )
        .await
    }

    /// Pauses a running timer.
    pub async fn pause(&self) -> Result<(), TimerError> {
        self.ensure_not_reentrant(
            "pause() cannot be awaited from the timer's active callback; use request_pause().",
        )?;
        self.request_pause().await
    }

    /// Requests that the current run pause after the current callback or sleep edge.
    pub async fn request_pause(&self) -> Result<(), TimerError> {
        let _run_id = self
            .active_run_id()
            .await
            .ok_or_else(TimerError::not_running)?;
        let mut state = self.inner.state.lock().await;
        if *state != TimerState::Running {
            return Err(TimerError::not_running());
        }

        *state = TimerState::Paused;
        drop(state);

        self.send_command(TimerCommand::Pause).await;

        #[cfg(feature = "logging")]
        debug!("Timer paused.");

        Ok(())
    }

    /// Resumes a paused timer.
    pub async fn resume(&self) -> Result<(), TimerError> {
        self.ensure_not_reentrant(
            "resume() cannot be awaited from the timer's active callback; use request_resume().",
        )?;
        self.request_resume().await
    }

    /// Requests that a paused timer resume.
    pub async fn request_resume(&self) -> Result<(), TimerError> {
        let _run_id = self
            .active_run_id()
            .await
            .ok_or_else(TimerError::not_paused)?;
        let mut state = self.inner.state.lock().await;
        if *state != TimerState::Paused {
            return Err(TimerError::not_paused());
        }

        *state = TimerState::Running;
        drop(state);

        self.send_command(TimerCommand::Resume).await;

        #[cfg(feature = "logging")]
        debug!("Timer resumed.");

        Ok(())
    }

    /// Stops the timer after the current callback finishes.
    pub async fn stop(&self) -> Result<TimerOutcome, TimerError> {
        self.ensure_not_reentrant(
            "stop() cannot be awaited from the timer's active callback; use request_stop().",
        )?;
        let run_id = self
            .active_run_id()
            .await
            .ok_or_else(TimerError::not_running)?;
        self.request_stop().await?;
        self.join_run(run_id).await
    }

    /// Requests a graceful stop without waiting for the outcome.
    pub async fn request_stop(&self) -> Result<(), TimerError> {
        self.active_run_id()
            .await
            .ok_or_else(TimerError::not_running)?;
        self.send_command(TimerCommand::Stop).await;
        Ok(())
    }

    /// Cancels the timer immediately and aborts the callback task.
    pub async fn cancel(&self) -> Result<TimerOutcome, TimerError> {
        self.ensure_not_reentrant(
            "cancel() cannot be awaited from the timer's active callback; use request_cancel().",
        )?;
        self.cancel_with_reason(TimerFinishReason::Cancelled).await
    }

    /// Requests cancellation without aborting the active callback task.
    pub async fn request_cancel(&self) -> Result<(), TimerError> {
        self.active_run_id()
            .await
            .ok_or_else(TimerError::not_running)?;
        self.send_command(TimerCommand::Cancel).await;
        Ok(())
    }

    /// Adjusts the interval of a running or paused timer.
    pub async fn adjust_interval(&self, new_interval: Duration) -> Result<(), TimerError> {
        self.ensure_not_reentrant(
            "adjust_interval() cannot be awaited from the timer's active callback; use request_adjust_interval().",
        )?;
        self.request_adjust_interval(new_interval).await
    }

    /// Requests an interval adjustment for the current run.
    pub async fn request_adjust_interval(&self, new_interval: Duration) -> Result<(), TimerError> {
        if new_interval.is_zero() {
            return Err(TimerError::invalid_parameter(
                "Interval must be greater than zero.",
            ));
        }

        let run_id = self
            .active_run_id()
            .await
            .ok_or_else(TimerError::not_running)?;
        *self.inner.interval.lock().await = new_interval;
        self.send_command(TimerCommand::SetInterval(new_interval))
            .await;
        runtime::emit_event(
            &self.inner,
            TimerEvent::IntervalAdjusted {
                run_id,
                interval: new_interval,
            },
        );

        #[cfg(feature = "logging")]
        debug!("Timer interval adjusted.");

        Ok(())
    }

    /// Waits for the current run and returns the completed outcome.
    pub async fn join(&self) -> Result<TimerOutcome, TimerError> {
        self.ensure_not_reentrant(
            "join() cannot be awaited from the timer's active callback; use completion().wait() from another task instead.",
        )?;
        if let Some(run_id) = self.active_run_id().await {
            return self.join_run(run_id).await;
        }

        self.inner
            .last_outcome
            .lock()
            .await
            .clone()
            .ok_or_else(TimerError::not_running)
    }

    /// Waits until the current run has completed or been stopped.
    pub async fn wait(&self) {
        let _ = self.join().await;
    }

    /// Gets the timer's statistics for the current or most recent run.
    pub async fn get_statistics(&self) -> TimerStatistics {
        self.inner.statistics.lock().await.clone()
    }

    /// Gets the current state of the timer.
    pub async fn get_state(&self) -> TimerState {
        *self.inner.state.lock().await
    }

    /// Gets the timer interval for the current or next run.
    pub async fn get_interval(&self) -> Duration {
        *self.inner.interval.lock().await
    }

    /// Gets the configured expiration count for the current or next run.
    pub async fn get_expiration_count(&self) -> Option<usize> {
        *self.inner.expiration_count.lock().await
    }

    /// Gets the most recent callback error observed for the current or most recent run.
    pub async fn get_last_error(&self) -> Option<TimerError> {
        self.inner.statistics.lock().await.last_error.clone()
    }

    /// Gets the most recent completed run outcome.
    pub async fn last_outcome(&self) -> Option<TimerOutcome> {
        self.inner.last_outcome.lock().await.clone()
    }

    /// Enables or disables broadcast event emission for future runtime events.
    pub fn set_events_enabled(&self, enabled: bool) {
        self.inner.events_enabled.store(enabled, Ordering::SeqCst);
    }

    async fn start_internal<F>(
        &self,
        interval: Duration,
        initial_delay: Option<Duration>,
        callback_timeout: Option<Duration>,
        retry_policy: Option<RetryPolicy>,
        callback: F,
        recurring: bool,
        expiration_count: Option<usize>,
        start_paused: bool,
    ) -> Result<u64, TimerError>
    where
        F: TimerCallback + 'static,
    {
        if interval.is_zero() {
            return Err(TimerError::invalid_parameter(
                "Interval must be greater than zero.",
            ));
        }

        if recurring && matches!(expiration_count, Some(0)) {
            return Err(TimerError::invalid_parameter(
                "Expiration count must be greater than zero.",
            ));
        }

        if initial_delay.is_some_and(|delay| delay.is_zero()) {
            return Err(TimerError::invalid_parameter(
                "Initial delay must be greater than zero.",
            ));
        }

        if callback_timeout.is_some_and(|timeout| timeout.is_zero()) {
            return Err(TimerError::invalid_parameter(
                "Callback timeout must be greater than zero.",
            ));
        }

        self.ensure_not_reentrant(
            "starting a new run from the timer's active callback is not supported; spawn a separate task instead.",
        )?;

        let _ = self.cancel_with_reason(TimerFinishReason::Replaced).await;

        let run_id = self.inner.next_run_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::unbounded_channel();

        {
            *self.inner.state.lock().await = if start_paused {
                TimerState::Paused
            } else {
                TimerState::Running
            };
            *self.inner.command_tx.lock().await = Some(tx);
            *self.inner.interval.lock().await = interval;
            *self.inner.expiration_count.lock().await = expiration_count;
            *self.inner.statistics.lock().await = TimerStatistics::default();
            *self.inner.last_outcome.lock().await = None;
            self.inner.completion_tx.send_replace(None);
        }
        self.inner.active_run_id.store(run_id, Ordering::SeqCst);

        runtime::emit_event(
            &self.inner,
            TimerEvent::Started {
                run_id,
                interval,
                recurring,
                expiration_count,
            },
        );

        let inner = Arc::clone(&self.inner);
        let handle = self.inner.runtime.spawn(async move {
            let scoped_inner = Arc::clone(&inner);
            runtime::with_run_context(&scoped_inner, run_id, async move {
                runtime::run_timer(
                    inner,
                    run_id,
                    interval,
                    initial_delay,
                    callback_timeout,
                    retry_policy,
                    recurring,
                    expiration_count,
                    callback,
                    rx,
                )
                .await;
            })
            .await;
        });

        *self.inner.handle.lock().await = Some(handle);

        #[cfg(feature = "logging")]
        debug!("Timer started.");

        Ok(run_id)
    }

    async fn active_run_id(&self) -> Option<u64> {
        match self.inner.active_run_id.load(Ordering::SeqCst) {
            0 => None,
            run_id => Some(run_id),
        }
    }

    async fn send_command(&self, command: TimerCommand) {
        if let Some(tx) = self.inner.command_tx.lock().await.as_ref() {
            let _ = tx.send(command);
        }
    }

    fn ensure_not_reentrant(&self, message: &'static str) -> Result<(), TimerError> {
        if runtime::is_current_run(&self.inner) {
            return Err(TimerError::reentrant_operation(message));
        }

        Ok(())
    }

    async fn cancel_with_reason(
        &self,
        reason: TimerFinishReason,
    ) -> Result<TimerOutcome, TimerError> {
        let run_id = self
            .active_run_id()
            .await
            .ok_or_else(TimerError::not_running)?;

        let _ = self.inner.command_tx.lock().await.take();
        let handle = self.inner.handle.lock().await.take();
        *self.inner.state.lock().await = TimerState::Stopped;

        if let Some(handle) = handle {
            handle.abort();
            let _ = handle.await;
        }

        let statistics = self.get_statistics().await;
        let outcome = TimerOutcome {
            run_id,
            reason,
            statistics,
        };

        runtime::finish_run(&self.inner, outcome.clone()).await;
        Ok(outcome)
    }

    async fn join_run(&self, run_id: u64) -> Result<TimerOutcome, TimerError> {
        let mut completion_rx = self.inner.completion_tx.subscribe();

        loop {
            if let Some(outcome) = completion_rx.borrow().clone() {
                if outcome.run_id == run_id {
                    return Ok(outcome);
                }
            }

            if completion_rx.changed().await.is_err() {
                return completion_rx
                    .borrow()
                    .clone()
                    .ok_or_else(TimerError::not_running);
            }
        }
    }
}

impl TimerBuilder {
    /// Creates a builder for a one-time timer.
    pub fn once(delay: Duration) -> Self {
        Self {
            kind: TimerKind::Once(delay),
            expiration_count: None,
            callback_timeout: None,
            retry_policy: None,
            start_paused: false,
            events_enabled: true,
        }
    }

    /// Creates a builder for a recurring timer.
    pub fn recurring(interval: Duration) -> Self {
        Self {
            kind: TimerKind::Recurring {
                interval,
                initial_delay: None,
            },
            expiration_count: None,
            callback_timeout: None,
            retry_policy: None,
            start_paused: false,
            events_enabled: true,
        }
    }

    /// Sets the expiration count for a recurring timer.
    pub fn expiration_count(mut self, expiration_count: usize) -> Self {
        self.expiration_count = Some(expiration_count);
        self
    }

    /// Sets an initial delay before the first recurring execution.
    pub fn initial_delay(mut self, initial_delay: Duration) -> Self {
        if let TimerKind::Recurring {
            initial_delay: builder_initial_delay,
            ..
        } = &mut self.kind
        {
            *builder_initial_delay = Some(initial_delay);
        }

        self
    }

    /// Sets a timeout for each callback execution.
    pub fn callback_timeout(mut self, callback_timeout: Duration) -> Self {
        self.callback_timeout = Some(callback_timeout);
        self
    }

    /// Retries failed callback executions according to the provided policy.
    pub fn retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = Some(retry_policy);
        self
    }

    /// Retries failed callback executions up to `max_retries` times.
    pub fn max_retries(mut self, max_retries: usize) -> Self {
        self.retry_policy = Some(RetryPolicy::new(max_retries));
        self
    }

    /// Starts the timer in the paused state.
    pub fn paused_start(mut self) -> Self {
        self.start_paused = true;
        self
    }

    /// Disables broadcast event emission for the timer.
    pub fn with_events_disabled(mut self) -> Self {
        self.events_enabled = false;
        self
    }

    /// Starts the configured timer and returns the handle.
    pub async fn start<F>(self, callback: F) -> Result<Timer, TimerError>
    where
        F: TimerCallback + 'static,
    {
        let timer = Timer::new_with_events(self.events_enabled);
        if self.start_paused {
            *timer.inner.state.lock().await = TimerState::Paused;
        }

        match self.kind {
            TimerKind::Once(delay) => {
                let _ = timer
                    .start_internal(
                        delay,
                        None,
                        self.callback_timeout,
                        self.retry_policy,
                        callback,
                        false,
                        None,
                        self.start_paused,
                    )
                    .await?;
            }
            TimerKind::Recurring {
                interval,
                initial_delay,
            } => {
                let _ = timer
                    .start_internal(
                        interval,
                        initial_delay,
                        self.callback_timeout,
                        self.retry_policy,
                        callback,
                        true,
                        self.expiration_count,
                        self.start_paused,
                    )
                    .await?;
            }
        }
        Ok(timer)
    }
}
