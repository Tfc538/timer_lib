use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use timer_lib::{Timer, TimerFinishReason, TimerRegistry};
use tokio::task::yield_now;
use tokio::time::{advance, timeout};

async fn settle() {
    for _ in 0..8 {
        yield_now().await;
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn lagged_event_subscriber_can_still_observe_finished() {
    let timer = Timer::new();
    let mut events = timer.subscribe();

    let run_id = timer
        .start_recurring(Duration::from_secs(1), || async { Ok(()) }, Some(80))
        .await
        .unwrap();

    advance(Duration::from_secs(80)).await;
    settle().await;

    let outcome = events.wait_finished().await.unwrap();
    assert_eq!(outcome.run_id, run_id);
    assert_eq!(outcome.reason, TimerFinishReason::Completed);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn join_is_idempotent_after_cancel() {
    let timer = Timer::new();
    let run_id = timer
        .start_recurring(Duration::from_secs(30), || async { Ok(()) }, None)
        .await
        .unwrap();

    let cancelled = timer.cancel().await.unwrap();
    let joined_again = timer.join().await.unwrap();

    assert_eq!(cancelled.run_id, run_id);
    assert_eq!(cancelled.reason, TimerFinishReason::Cancelled);
    assert_eq!(joined_again, cancelled);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn removed_timer_keeps_running_after_leaving_registry() {
    let registry = TimerRegistry::new();
    let (timer_id, timer) = registry
        .start_once(Duration::from_secs(2), || async { Ok(()) })
        .await
        .unwrap();

    let removed = registry.remove(timer_id).await.unwrap();
    assert!(registry.get(timer_id).await.is_none());

    advance(Duration::from_secs(2)).await;
    settle().await;

    let original_outcome = timer.join().await.unwrap();
    let removed_outcome = removed.join().await.unwrap();

    assert_eq!(original_outcome.reason, TimerFinishReason::Completed);
    assert_eq!(removed_outcome, original_outcome);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_stop_cancel_and_join_do_not_deadlock() {
    let timer = Timer::new();
    let run_id = timer
        .start_recurring(Duration::from_millis(50), || async { Ok(()) }, None)
        .await
        .unwrap();

    let stop_task = {
        let timer = timer.clone();
        tokio::spawn(async move { timer.stop().await })
    };
    let cancel_task = {
        let timer = timer.clone();
        tokio::spawn(async move { timer.cancel().await })
    };
    let join_task = {
        let timer = timer.clone();
        tokio::spawn(async move { timer.join().await })
    };

    let stop_result = timeout(Duration::from_secs(1), stop_task)
        .await
        .unwrap()
        .unwrap();
    let cancel_result = timeout(Duration::from_secs(1), cancel_task)
        .await
        .unwrap()
        .unwrap();
    let join_result = timeout(Duration::from_secs(1), join_task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();

    let mut success_count = 0;
    if let Ok(outcome) = &stop_result {
        success_count += 1;
        assert_eq!(outcome.run_id, run_id);
        assert!(matches!(
            outcome.reason,
            TimerFinishReason::Stopped | TimerFinishReason::Cancelled
        ));
    }
    if let Ok(outcome) = &cancel_result {
        success_count += 1;
        assert_eq!(outcome.run_id, run_id);
        assert!(matches!(
            outcome.reason,
            TimerFinishReason::Stopped | TimerFinishReason::Cancelled
        ));
    }

    assert!(success_count >= 1);
    assert_eq!(join_result.run_id, run_id);
    assert!(matches!(
        join_result.reason,
        TimerFinishReason::Stopped | TimerFinishReason::Cancelled
    ));
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn callback_can_request_stop_on_itself_without_deadlocking() {
    let timer = Timer::new();
    let timer_for_callback = timer.clone();
    let saw_reentrant_error = Arc::new(AtomicBool::new(false));
    let saw_reentrant_error_in_callback = Arc::clone(&saw_reentrant_error);

    timer
        .start_recurring(
            Duration::from_secs(1),
            move || {
                let timer = timer_for_callback.clone();
                let saw_reentrant_error = Arc::clone(&saw_reentrant_error_in_callback);
                async move {
                    let err = timer.stop().await.unwrap_err();
                    saw_reentrant_error.store(err.is_reentrant_operation(), Ordering::SeqCst);
                    timer.request_stop().await.unwrap();
                    Ok(())
                }
            },
            None,
        )
        .await
        .unwrap();

    advance(Duration::from_secs(1)).await;
    settle().await;

    let outcome = timer.join().await.unwrap();
    assert!(saw_reentrant_error.load(Ordering::SeqCst));
    assert_eq!(outcome.reason, TimerFinishReason::Stopped);
    assert_eq!(outcome.statistics.execution_count, 1);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn callback_cannot_restart_its_own_timer_inline() {
    let timer = Timer::new();
    let timer_for_callback = timer.clone();
    let saw_reentrant_error = Arc::new(AtomicBool::new(false));
    let saw_reentrant_error_in_callback = Arc::clone(&saw_reentrant_error);

    timer
        .start_once(Duration::from_secs(1), move || {
            let timer = timer_for_callback.clone();
            let saw_reentrant_error = Arc::clone(&saw_reentrant_error_in_callback);
            async move {
                let err = timer
                    .start_once(Duration::from_secs(5), || async { Ok(()) })
                    .await
                    .unwrap_err();
                saw_reentrant_error.store(err.is_reentrant_operation(), Ordering::SeqCst);
                Ok(())
            }
        })
        .await
        .unwrap();

    advance(Duration::from_secs(1)).await;
    settle().await;

    let outcome = timer.join().await.unwrap();
    assert!(saw_reentrant_error.load(Ordering::SeqCst));
    assert_eq!(outcome.reason, TimerFinishReason::Completed);
    assert_eq!(outcome.statistics.execution_count, 1);
}
