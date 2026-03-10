use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;
use tokio::task::yield_now;
use tokio::time::{advance, Instant};

struct CountingCallback {
    executions: Arc<AtomicUsize>,
    fail: bool,
}

#[async_trait]
impl TimerCallback for CountingCallback {
    async fn execute(&self) -> Result<(), TimerError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            Err(TimerError::callback_failed("forced failure"))
        } else {
            Ok(())
        }
    }
}

async fn settle() {
    for _ in 0..5 {
        yield_now().await;
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn timer_starts_stopped() {
    let timer = Timer::new();

    assert_eq!(timer.get_state().await, TimerState::Stopped);
    assert_eq!(timer.get_interval().await, Duration::ZERO);
    assert_eq!(timer.get_expiration_count().await, None);
    assert_eq!(timer.get_statistics().await, TimerStatistics::default());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn one_shot_timer_returns_completed_outcome() {
    let executions = Arc::new(AtomicUsize::new(0));
    let timer = Timer::new();

    let run_id = timer
        .start_once(
            Duration::from_secs(5),
            CountingCallback {
                executions: Arc::clone(&executions),
                fail: false,
            },
        )
        .await
        .unwrap();

    advance(Duration::from_secs(5)).await;
    settle().await;

    let outcome = timer.join().await.unwrap();
    assert_eq!(outcome.run_id, run_id);
    assert_eq!(outcome.reason, TimerFinishReason::Completed);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn stop_is_graceful_and_cancel_is_immediate() {
    let executions = Arc::new(AtomicUsize::new(0));
    let timer = Timer::new();

    timer
        .start_recurring(
            RecurringSchedule::new(Duration::from_secs(1)),
            CountingCallback {
                executions: Arc::clone(&executions),
                fail: false,
            },
        )
        .await
        .unwrap();

    yield_now().await;
    advance(Duration::from_secs(1)).await;
    settle().await;
    let stopped = timer.stop().await.unwrap();
    assert_eq!(stopped.reason, TimerFinishReason::Stopped);
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    timer
        .start_recurring(RecurringSchedule::new(Duration::from_secs(10)), || async {
            Ok(())
        })
        .await
        .unwrap();
    let cancelled = timer.cancel().await.unwrap();
    assert_eq!(cancelled.reason, TimerFinishReason::Cancelled);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn replacing_a_run_records_replaced_outcome() {
    let timer = Timer::new();

    let first_run = timer
        .start_recurring(RecurringSchedule::new(Duration::from_secs(10)), || async {
            Ok(())
        })
        .await
        .unwrap();

    let second_run = timer
        .start_once(Duration::from_secs(1), || async { Ok(()) })
        .await
        .unwrap();

    assert_ne!(first_run, second_run);
    let outcome = timer.join().await.unwrap();
    assert_eq!(outcome.run_id, second_run);
    assert_eq!(outcome.reason, TimerFinishReason::Completed);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn interval_adjustments_apply_to_future_ticks() {
    let executions = Arc::new(AtomicUsize::new(0));
    let timer = Timer::new();

    timer
        .start_recurring(
            RecurringSchedule::new(Duration::from_secs(5)),
            CountingCallback {
                executions: Arc::clone(&executions),
                fail: false,
            },
        )
        .await
        .unwrap();

    yield_now().await;
    advance(Duration::from_secs(5)).await;
    settle().await;
    timer
        .adjust_interval(Duration::from_secs(30))
        .await
        .unwrap();
    settle().await;
    advance(Duration::from_secs(29)).await;
    settle().await;
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(executions.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn events_are_emitted_for_key_lifecycle_changes() {
    let timer = Timer::new();
    let mut events = timer.subscribe();

    let run_id = timer
        .start_recurring(
            RecurringSchedule::new(Duration::from_secs(1)).with_expiration_count(1),
            || async { Ok(()) },
        )
        .await
        .unwrap();

    assert_eq!(
        events.wait_started().await,
        Some(TimerEvent::Started {
            run_id,
            interval: Duration::from_secs(1),
            recurring: true,
            expiration_count: Some(1),
        })
    );

    advance(Duration::from_secs(1)).await;
    settle().await;

    assert!(matches!(
        events.wait_tick().await,
        Some(TimerEvent::Tick { run_id: seen, .. }) if seen == run_id
    ));
    let finished = events.wait_finished().await.unwrap();
    assert_eq!(finished.run_id, run_id);
    assert_eq!(finished.reason, TimerFinishReason::Completed);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn builder_starts_recurring_timers_with_less_boilerplate() {
    let executions = Arc::new(AtomicUsize::new(0));
    let timer =
        Timer::recurring(RecurringSchedule::new(Duration::from_secs(1)).with_expiration_count(2))
            .start(CountingCallback {
                executions: Arc::clone(&executions),
                fail: false,
            })
            .await
            .unwrap();

    advance(Duration::from_secs(2)).await;
    settle().await;

    let outcome = timer.join().await.unwrap();
    assert_eq!(outcome.reason, TimerFinishReason::Completed);
    assert_eq!(executions.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn recurring_timers_can_delay_the_first_tick() {
    let executions = Arc::new(AtomicUsize::new(0));
    let timer = Timer::new();

    timer
        .start_recurring(
            RecurringSchedule::new(Duration::from_secs(5))
                .with_initial_delay(Duration::from_secs(2))
                .with_expiration_count(2),
            CountingCallback {
                executions: Arc::clone(&executions),
                fail: false,
            },
        )
        .await
        .unwrap();
    settle().await;

    advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(executions.load(Ordering::SeqCst), 0);

    advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    advance(Duration::from_secs(4)).await;
    settle().await;
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(executions.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn completion_subscription_is_lossless() {
    let timer = Timer::new();
    let mut completion = timer.completion();

    let run_id = timer
        .start_once(Duration::from_secs(2), || async { Ok(()) })
        .await
        .unwrap();

    advance(Duration::from_secs(2)).await;
    settle().await;

    let outcome = completion.wait_for_run(run_id).await.unwrap();
    assert_eq!(outcome.run_id, run_id);
    assert_eq!(outcome.reason, TimerFinishReason::Completed);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn completion_wait_advances_to_the_next_unseen_outcome() {
    let timer = Timer::new();
    let mut completion = timer.completion();

    let first_run = timer
        .start_once(Duration::from_secs(1), || async { Ok(()) })
        .await
        .unwrap();

    advance(Duration::from_secs(1)).await;
    settle().await;

    let first_outcome = completion.wait().await.unwrap();
    assert_eq!(first_outcome.run_id, first_run);

    let second_wait = tokio::spawn(async move { completion.wait().await });
    let second_run = timer
        .start_once(Duration::from_secs(1), || async { Ok(()) })
        .await
        .unwrap();

    advance(Duration::from_secs(1)).await;
    settle().await;

    let second_outcome = second_wait.await.unwrap().unwrap();
    assert_eq!(second_outcome.run_id, second_run);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn paused_builder_start_waits_for_resume() {
    let timer =
        Timer::recurring(RecurringSchedule::new(Duration::from_secs(1)).with_expiration_count(1))
            .paused_start()
            .start(|| async { Ok(()) })
            .await
            .unwrap();

    advance(Duration::from_secs(5)).await;
    settle().await;
    assert_eq!(timer.get_statistics().await.execution_count, 0);

    timer.resume().await.unwrap();
    advance(Duration::from_secs(1)).await;
    settle().await;

    assert_eq!(
        timer.join().await.unwrap().reason,
        TimerFinishReason::Completed
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn builder_initial_delay_controls_the_first_recurring_tick() {
    let executions = Arc::new(AtomicUsize::new(0));
    let timer = Timer::recurring(
        RecurringSchedule::new(Duration::from_secs(3))
            .with_initial_delay(Duration::from_secs(1))
            .with_expiration_count(2),
    )
    .start(CountingCallback {
        executions: Arc::clone(&executions),
        fail: false,
    })
    .await
    .unwrap();
    settle().await;

    advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    advance(Duration::from_secs(2)).await;
    settle().await;
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    advance(Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(executions.load(Ordering::SeqCst), 2);
    assert_eq!(
        timer.join().await.unwrap().reason,
        TimerFinishReason::Completed
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn callback_timeout_counts_as_a_failed_execution() {
    let timer = Timer::once(Duration::from_secs(1))
        .callback_timeout(Duration::from_secs(2))
        .start(|| async {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok::<(), TimerError>(())
        })
        .await
        .unwrap();
    settle().await;

    advance(Duration::from_secs(1)).await;
    settle().await;
    advance(Duration::from_secs(2)).await;
    settle().await;

    let outcome = timer.join().await.unwrap();
    assert_eq!(outcome.reason, TimerFinishReason::Completed);
    assert_eq!(outcome.statistics.execution_count, 1);
    assert_eq!(outcome.statistics.failed_executions, 1);
    assert_eq!(outcome.statistics.successful_executions, 0);
    assert!(outcome
        .statistics
        .last_error
        .as_ref()
        .is_some_and(TimerError::is_callback_timed_out));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn retry_policy_retries_failed_callbacks_before_succeeding() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_for_callback = Arc::clone(&attempts);
    let timer = Timer::once(Duration::from_secs(1))
        .max_retries(2)
        .start(move || {
            let attempts = Arc::clone(&attempts_for_callback);
            async move {
                if attempts.fetch_add(1, Ordering::SeqCst) < 2 {
                    Err(TimerError::callback_failed("try again"))
                } else {
                    Ok(())
                }
            }
        })
        .await
        .unwrap();
    settle().await;

    advance(Duration::from_secs(1)).await;
    settle().await;

    let outcome = timer.join().await.unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(outcome.statistics.execution_count, 1);
    assert_eq!(outcome.statistics.failed_executions, 2);
    assert_eq!(outcome.statistics.successful_executions, 1);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn event_suppression_can_be_enabled_from_the_builder() {
    let timer = Timer::once(Duration::from_secs(1))
        .with_events_disabled()
        .start(|| async { Ok(()) })
        .await
        .unwrap();
    let mut events = timer.subscribe();

    advance(Duration::from_secs(1)).await;
    settle().await;

    assert!(events.try_recv().is_none());
    assert_eq!(
        timer.join().await.unwrap().reason,
        TimerFinishReason::Completed
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn event_helpers_wait_for_pause_resume_and_stop() {
    let timer = Timer::new();
    let mut events = timer.subscribe();

    timer
        .start_recurring(RecurringSchedule::new(Duration::from_secs(2)), || async {
            Ok(())
        })
        .await
        .unwrap();
    settle().await;

    timer.pause().await.unwrap();
    assert!(matches!(
        events.wait_paused().await,
        Some(TimerEvent::Paused { .. })
    ));

    timer.resume().await.unwrap();
    assert!(matches!(
        events.wait_resumed().await,
        Some(TimerEvent::Resumed { .. })
    ));

    let stopped = timer.stop().await.unwrap();
    let seen = events.wait_stopped().await.unwrap();
    assert_eq!(seen, stopped);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn event_helpers_wait_for_cancelled_outcomes() {
    let timer = Timer::new();
    let mut events = timer.subscribe();

    timer
        .start_recurring(RecurringSchedule::new(Duration::from_secs(5)), || async {
            Ok(())
        })
        .await
        .unwrap();
    settle().await;

    let cancelled = timer.cancel().await.unwrap();
    let seen = events.wait_cancelled().await.unwrap();
    assert_eq!(seen, cancelled);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn fixed_rate_and_fixed_delay_schedules_diverge_under_slow_callbacks() {
    let fixed_delay_starts = Arc::new(StdMutex::new(Vec::new()));
    let fixed_rate_starts = Arc::new(StdMutex::new(Vec::new()));
    let fixed_delay_base = Instant::now();
    let fixed_rate_base = fixed_delay_base;

    let fixed_delay_timer = Timer::new();
    let fixed_delay_starts_for_callback = Arc::clone(&fixed_delay_starts);
    fixed_delay_timer
        .start_recurring(
            RecurringSchedule::new(Duration::from_secs(5))
                .fixed_delay()
                .with_expiration_count(2),
            move || {
                let starts = Arc::clone(&fixed_delay_starts_for_callback);
                async move {
                    starts
                        .lock()
                        .unwrap()
                        .push((Instant::now() - fixed_delay_base).as_secs());
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Ok::<(), TimerError>(())
                }
            },
        )
        .await
        .unwrap();

    let fixed_rate_timer = Timer::new();
    let fixed_rate_starts_for_callback = Arc::clone(&fixed_rate_starts);
    fixed_rate_timer
        .start_recurring(
            RecurringSchedule::new(Duration::from_secs(5))
                .fixed_rate()
                .with_expiration_count(2),
            move || {
                let starts = Arc::clone(&fixed_rate_starts_for_callback);
                async move {
                    starts
                        .lock()
                        .unwrap()
                        .push((Instant::now() - fixed_rate_base).as_secs());
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Ok::<(), TimerError>(())
                }
            },
        )
        .await
        .unwrap();
    settle().await;

    advance(Duration::from_secs(5)).await;
    settle().await;

    assert_eq!(*fixed_delay_starts.lock().unwrap(), vec![5]);
    assert_eq!(*fixed_rate_starts.lock().unwrap(), vec![5]);

    advance(Duration::from_secs(2)).await;
    settle().await;

    advance(Duration::from_secs(3)).await;
    settle().await;

    assert_eq!(*fixed_rate_starts.lock().unwrap(), vec![5, 10]);
    assert_eq!(*fixed_delay_starts.lock().unwrap(), vec![5]);

    advance(Duration::from_secs(2)).await;
    settle().await;

    assert_eq!(*fixed_delay_starts.lock().unwrap(), vec![5, 12]);
    assert_eq!(
        fixed_delay_timer.join().await.unwrap().reason,
        TimerFinishReason::Completed
    );
    assert_eq!(
        fixed_rate_timer.join().await.unwrap().reason,
        TimerFinishReason::Completed
    );
}
