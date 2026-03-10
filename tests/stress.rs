use std::sync::{
    atomic::{AtomicUsize, Ordering},
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
async fn many_timers_complete_under_registry_load() {
    const TIMER_COUNT: usize = 512;

    let registry = TimerRegistry::new();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut timers = Vec::with_capacity(TIMER_COUNT);

    for index in 0..TIMER_COUNT {
        let executions = Arc::clone(&executions);
        let (timer_id, timer) = registry
            .start_once(Duration::from_millis((index as u64) + 1), move || {
                let executions = Arc::clone(&executions);
                async move {
                    executions.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }
            })
            .await
            .unwrap();

        assert!(registry.get(timer_id).await.is_some());
        timers.push(timer);
    }

    advance(Duration::from_millis(TIMER_COUNT as u64)).await;
    settle().await;

    for timer in timers {
        let outcome = timer.join().await.unwrap();
        assert_eq!(outcome.reason, TimerFinishReason::Completed);
    }

    assert_eq!(executions.load(Ordering::SeqCst), TIMER_COUNT);
    assert!(registry.active_ids().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_joiners_see_the_same_completed_outcome() {
    let timer = Timer::new();
    let run_id = timer
        .start_once(Duration::from_millis(20), || async { Ok(()) })
        .await
        .unwrap();

    let joiner_a = {
        let timer = timer.clone();
        tokio::spawn(async move { timer.join().await.unwrap() })
    };
    let joiner_b = {
        let timer = timer.clone();
        tokio::spawn(async move { timer.join().await.unwrap() })
    };

    let outcome_a = timeout(Duration::from_secs(1), joiner_a)
        .await
        .unwrap()
        .unwrap();
    let outcome_b = timeout(Duration::from_secs(1), joiner_b)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(outcome_a.run_id, run_id);
    assert_eq!(outcome_b.run_id, run_id);
    assert_eq!(outcome_a.reason, TimerFinishReason::Completed);
    assert_eq!(outcome_b.reason, TimerFinishReason::Completed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn registry_cancel_all_stops_many_recurring_timers_concurrently() {
    const TIMER_COUNT: usize = 512;

    let registry = TimerRegistry::new();
    let mut timers = Vec::with_capacity(TIMER_COUNT);

    for _ in 0..TIMER_COUNT {
        let (timer_id, timer) = registry
            .start_recurring(Duration::from_millis(50), || async { Ok(()) }, None)
            .await
            .unwrap();
        assert!(registry.get(timer_id).await.is_some());
        timers.push(timer);
    }

    registry.cancel_all().await;

    for timer in timers {
        let outcome = timeout(Duration::from_secs(1), timer.join())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(outcome.reason, TimerFinishReason::Cancelled);
    }

    assert!(registry.active_ids().await.is_empty());
}
