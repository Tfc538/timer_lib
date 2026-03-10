use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time;
use tokio::time::Instant;

#[cfg(feature = "logging")]
use log::error;

use super::{
    RecurringCadence, RetryPolicy, RunConfig, TimerCallback, TimerCommand, TimerEvent,
    TimerFinishReason, TimerInner, TimerOutcome, TimerState,
};

tokio::task_local! {
    static ACTIVE_RUN_CONTEXT: ActiveRunContext;
}

#[derive(Clone, Copy)]
struct ActiveRunContext {
    timer_key: usize,
    run_id: u64,
}

struct RunSnapshot {
    run_id: u64,
    started_at: Instant,
    tick_count: usize,
    success_count: usize,
    failure_count: usize,
    last_error: Option<crate::errors::TimerError>,
}

pub(super) async fn with_run_context<F>(inner: &Arc<TimerInner>, run_id: u64, future: F)
where
    F: Future<Output = ()>,
{
    ACTIVE_RUN_CONTEXT
        .scope(
            ActiveRunContext {
                timer_key: timer_key(inner),
                run_id,
            },
            future,
        )
        .await;
}

pub(super) fn is_current_run(inner: &Arc<TimerInner>) -> bool {
    let active_run_id = inner
        .active_run_id
        .load(std::sync::atomic::Ordering::SeqCst);
    ACTIVE_RUN_CONTEXT
        .try_with(|ctx| ctx.timer_key == timer_key(inner) && ctx.run_id == active_run_id)
        .unwrap_or(false)
}

pub(super) async fn run_timer<F>(
    inner: Arc<TimerInner>,
    run_id: u64,
    config: RunConfig,
    callback: F,
    mut rx: mpsc::UnboundedReceiver<TimerCommand>,
) where
    F: TimerCallback + 'static,
{
    let started_at = inner.runtime.now();
    let first_delay = first_sleep_delay(&inner, &config);
    let mut tick_count = 0usize;
    let mut success_count = 0usize;
    let mut failure_count = 0usize;
    let mut current_interval = config.interval;
    let mut next_sleep = first_delay;
    let mut next_deadline = config.recurring.then_some(started_at + first_delay);
    let mut last_error = None;

    loop {
        match wait_while_paused(&inner, &mut rx, &mut current_interval).await {
            RunControl::Continue => {}
            RunControl::Finish(reason) => {
                let outcome = snapshot_outcome(
                    &inner,
                    reason,
                    RunSnapshot {
                        run_id,
                        started_at,
                        tick_count,
                        success_count,
                        failure_count,
                        last_error: last_error.clone(),
                    },
                )
                .await;
                finish_run(&inner, outcome).await;
                return;
            }
        }

        let sleep = if !config.recurring {
            if let Some(deadline) = config.start_deadline {
                inner.runtime.sleep_until(deadline)
            } else {
                inner.runtime.sleep(next_sleep)
            }
        } else {
            inner.runtime.sleep(next_sleep)
        };
        tokio::pin!(sleep);

        loop {
            tokio::select! {
                _ = &mut sleep => break,
                cmd = rx.recv() => match cmd {
                    Some(TimerCommand::Pause) => {
                        *inner.state.lock().await = TimerState::Paused;
                        emit_event(&inner, TimerEvent::Paused { run_id });
                        match wait_while_paused(&inner, &mut rx, &mut current_interval).await {
                            RunControl::Continue => {
                                reset_recurring_deadline(
                                    &inner,
                                    &config,
                                    &mut next_deadline,
                                    current_interval,
                                );
                                sleep.set(inner.runtime.sleep(current_interval));
                            }
                            RunControl::Finish(reason) => {
                                let outcome = snapshot_outcome(
                                    &inner,
                                    reason,
                                    RunSnapshot {
                                        run_id,
                                        started_at,
                                        tick_count,
                                        success_count,
                                        failure_count,
                                        last_error: last_error.clone(),
                                    },
                                )
                                .await;
                                finish_run(&inner, outcome).await;
                                return;
                            }
                        }
                    }
                    Some(TimerCommand::Cancel) => {
                        let outcome = snapshot_outcome(
                            &inner,
                            TimerFinishReason::Cancelled,
                            RunSnapshot {
                                run_id,
                                started_at,
                                tick_count,
                                success_count,
                                failure_count,
                                last_error: last_error.clone(),
                            },
                        )
                        .await;
                        finish_run(&inner, outcome).await;
                        return;
                    }
                    Some(TimerCommand::Resume) => {}
                    Some(TimerCommand::Stop) | None => {
                        let outcome = snapshot_outcome(
                            &inner,
                            TimerFinishReason::Stopped,
                            RunSnapshot {
                                run_id,
                                started_at,
                                tick_count,
                                success_count,
                                failure_count,
                                last_error: last_error.clone(),
                            },
                        )
                        .await;
                        finish_run(&inner, outcome).await;
                        return;
                    }
                    Some(TimerCommand::SetInterval(new_interval)) => {
                        current_interval = new_interval;
                        reset_recurring_deadline(
                            &inner,
                            &config,
                            &mut next_deadline,
                            current_interval,
                        );
                        sleep.set(inner.runtime.sleep(current_interval));
                    }
                }
            }
        }

        let max_attempts = config
            .retry_policy
            .map_or(1, |policy| policy.max_retries() + 1);
        let mut callback_succeeded = false;

        for attempt in 0..max_attempts {
            let callback_result = match config.callback_timeout {
                Some(timeout) => match time::timeout(timeout, callback.execute()).await {
                    Ok(result) => result,
                    Err(_) => Err(crate::errors::TimerError::callback_timed_out(timeout)),
                },
                None => callback.execute().await,
            };

            match callback_result {
                Ok(()) => {
                    success_count += 1;
                    callback_succeeded = true;
                    break;
                }
                Err(err) => {
                    #[cfg(feature = "logging")]
                    error!("Callback execution error: {}", err);
                    failure_count += 1;
                    last_error = Some(err.clone());

                    if attempt + 1 < max_attempts {
                        if let Some(backoff) = retry_backoff_delay(config.retry_policy, attempt + 1)
                        {
                            if !backoff.is_zero() {
                                inner.runtime.sleep(backoff).await;
                            }
                        }
                    }
                }
            }
        }

        if !callback_succeeded {
            #[cfg(feature = "logging")]
            if let Some(error) = &last_error {
                error!("Callback execution exhausted retries: {}", error);
            }
        }

        tick_count += 1;
        next_sleep = next_sleep_duration(&inner, &config, &mut next_deadline, current_interval);

        let statistics = update_statistics(
            &inner,
            started_at,
            tick_count,
            success_count,
            failure_count,
            last_error.clone(),
        )
        .await;

        if let Some(error) = statistics.last_error.clone() {
            emit_event(
                &inner,
                TimerEvent::CallbackFailed {
                    run_id,
                    error,
                    statistics: statistics.clone(),
                },
            );
        }

        emit_event(
            &inner,
            TimerEvent::Tick {
                run_id,
                statistics: statistics.clone(),
            },
        );

        match drain_post_tick_commands(&inner, &mut rx, &mut current_interval, run_id).await {
            RunControl::Continue => {}
            RunControl::Finish(reason) => {
                let outcome = TimerOutcome {
                    run_id,
                    reason,
                    statistics,
                };
                finish_run(&inner, outcome).await;
                return;
            }
        }

        if !config.recurring
            || config
                .expiration_count
                .is_some_and(|max_ticks| tick_count >= max_ticks)
        {
            let outcome = TimerOutcome {
                run_id,
                reason: TimerFinishReason::Completed,
                statistics,
            };
            finish_run(&inner, outcome).await;
            return;
        }
    }
}

fn next_sleep_duration(
    inner: &Arc<TimerInner>,
    config: &RunConfig,
    next_deadline: &mut Option<Instant>,
    current_interval: Duration,
) -> Duration {
    let base = match config.cadence {
        RecurringCadence::FixedDelay => current_interval,
        RecurringCadence::FixedRate => {
            let now = inner.runtime.now();
            let deadline = next_deadline.get_or_insert(now + current_interval);
            *deadline += current_interval;
            deadline.saturating_duration_since(now)
        }
    };

    apply_jitter(inner, base, config.jitter)
}

fn reset_recurring_deadline(
    inner: &Arc<TimerInner>,
    config: &RunConfig,
    next_deadline: &mut Option<Instant>,
    current_interval: Duration,
) {
    if config.recurring && config.cadence == RecurringCadence::FixedRate {
        *next_deadline = Some(inner.runtime.now() + current_interval);
    }
}

enum RunControl {
    Continue,
    Finish(TimerFinishReason),
}

async fn wait_while_paused(
    inner: &Arc<TimerInner>,
    rx: &mut mpsc::UnboundedReceiver<TimerCommand>,
    current_interval: &mut Duration,
) -> RunControl {
    loop {
        if *inner.state.lock().await != TimerState::Paused {
            return RunControl::Continue;
        }

        match rx.recv().await {
            Some(TimerCommand::Resume) => {
                *inner.state.lock().await = TimerState::Running;
                emit_event(
                    inner,
                    TimerEvent::Resumed {
                        run_id: inner
                            .active_run_id
                            .load(std::sync::atomic::Ordering::SeqCst),
                    },
                );
                return RunControl::Continue;
            }
            Some(TimerCommand::Cancel) => return RunControl::Finish(TimerFinishReason::Cancelled),
            Some(TimerCommand::Stop) => return RunControl::Finish(TimerFinishReason::Stopped),
            Some(TimerCommand::SetInterval(new_interval)) => {
                *current_interval = new_interval;
            }
            Some(TimerCommand::Pause) => {}
            None => return RunControl::Finish(TimerFinishReason::Cancelled),
        }
    }
}

async fn drain_post_tick_commands(
    inner: &Arc<TimerInner>,
    rx: &mut mpsc::UnboundedReceiver<TimerCommand>,
    current_interval: &mut Duration,
    run_id: u64,
) -> RunControl {
    loop {
        match rx.try_recv() {
            Ok(TimerCommand::Pause) => {
                *inner.state.lock().await = TimerState::Paused;
                emit_event(inner, TimerEvent::Paused { run_id });
                return wait_while_paused(inner, rx, current_interval).await;
            }
            Ok(TimerCommand::Resume) => {}
            Ok(TimerCommand::Cancel) => return RunControl::Finish(TimerFinishReason::Cancelled),
            Ok(TimerCommand::Stop) => return RunControl::Finish(TimerFinishReason::Stopped),
            Ok(TimerCommand::SetInterval(new_interval)) => {
                *current_interval = new_interval;
            }
            Err(mpsc::error::TryRecvError::Empty) => return RunControl::Continue,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                return RunControl::Finish(TimerFinishReason::Cancelled);
            }
        }
    }
}

async fn update_statistics(
    inner: &Arc<TimerInner>,
    started_at: Instant,
    tick_count: usize,
    success_count: usize,
    failure_count: usize,
    last_error: Option<crate::errors::TimerError>,
) -> super::TimerStatistics {
    let statistics = super::TimerStatistics {
        execution_count: tick_count,
        successful_executions: success_count,
        failed_executions: failure_count,
        elapsed_time: started_at.elapsed(),
        last_error,
    };

    *inner.statistics.lock().await = statistics.clone();
    statistics
}

fn first_sleep_delay(inner: &Arc<TimerInner>, config: &RunConfig) -> Duration {
    let base = match config.start_deadline {
        Some(deadline) => deadline.saturating_duration_since(inner.runtime.now()),
        None => config.initial_delay.unwrap_or(config.interval),
    };

    if config.recurring {
        apply_jitter(inner, base, config.jitter)
    } else {
        base
    }
}

fn retry_backoff_delay(retry_policy: Option<RetryPolicy>, retry_number: usize) -> Option<Duration> {
    retry_policy.map(|policy| policy.delay_for_retry(retry_number))
}

fn apply_jitter(inner: &Arc<TimerInner>, base: Duration, jitter: Option<Duration>) -> Duration {
    match jitter {
        Some(max_jitter) => base.saturating_add(inner.runtime.sample_jitter(max_jitter)),
        None => base,
    }
}

async fn snapshot_outcome(
    inner: &Arc<TimerInner>,
    reason: TimerFinishReason,
    snapshot: RunSnapshot,
) -> TimerOutcome {
    let statistics = update_statistics(
        inner,
        snapshot.started_at,
        snapshot.tick_count,
        snapshot.success_count,
        snapshot.failure_count,
        snapshot.last_error,
    )
    .await;

    TimerOutcome {
        run_id: snapshot.run_id,
        reason,
        statistics,
    }
}

pub(super) fn emit_event(inner: &Arc<TimerInner>, event: TimerEvent) {
    if !inner
        .events_enabled
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return;
    }

    let _ = inner.event_tx.send(event);
}

pub(super) async fn finish_run(inner: &Arc<TimerInner>, outcome: TimerOutcome) {
    inner
        .active_run_id
        .store(0, std::sync::atomic::Ordering::SeqCst);
    *inner.state.lock().await = TimerState::Stopped;
    *inner.command_tx.lock().await = None;
    *inner.handle.lock().await = None;
    *inner.last_outcome.lock().await = Some(outcome.clone());
    inner.completion_tx.send_replace(Some(outcome.clone()));

    emit_event(inner, TimerEvent::Finished(outcome));
}

fn timer_key(inner: &Arc<TimerInner>) -> usize {
    Arc::as_ptr(inner) as usize
}
