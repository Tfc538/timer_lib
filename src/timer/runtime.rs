use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

#[cfg(feature = "logging")]
use log::error;

use super::{
    TimerCallback, TimerCommand, TimerEvent, TimerFinishReason, TimerInner, TimerOutcome,
    TimerState,
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
    interval: Duration,
    initial_delay: Option<Duration>,
    recurring: bool,
    expiration_count: Option<usize>,
    callback: F,
    mut rx: mpsc::UnboundedReceiver<TimerCommand>,
) where
    F: TimerCallback + 'static,
{
    let started_at = inner.runtime.now();
    let mut tick_count = 0usize;
    let mut success_count = 0usize;
    let mut failure_count = 0usize;
    let mut current_interval = interval;
    let mut next_sleep = initial_delay.unwrap_or(interval);
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

        let sleep = inner.runtime.sleep(next_sleep);
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
                                sleep.as_mut().reset(Instant::now() + current_interval);
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
                        sleep.as_mut().reset(Instant::now() + current_interval);
                    }
                }
            }
        }

        match callback.execute().await {
            Ok(()) => {
                success_count += 1;
            }
            Err(err) => {
                #[cfg(feature = "logging")]
                error!("Callback execution error: {}", err);
                failure_count += 1;
                last_error = Some(err.clone());
            }
        }

        tick_count += 1;
        next_sleep = current_interval;

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

        if !recurring || expiration_count.is_some_and(|max_ticks| tick_count >= max_ticks) {
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
