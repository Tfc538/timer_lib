use std::time::Duration;

use timer_lib::{Timer, TimerEvent, TimerFinishReason, TimerRegistry};
use tokio::task::yield_now;
use tokio::time::advance;

async fn settle() {
    for _ in 0..5 {
        yield_now().await;
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn timer_closure_api_is_simple_to_use() {
    let timer = Timer::new();
    let run_id = timer
        .start_once(Duration::from_secs(1), || async { Ok(()) })
        .await
        .unwrap();

    advance(Duration::from_secs(1)).await;
    settle().await;

    let outcome = timer.join().await.unwrap();
    assert_eq!(outcome.run_id, run_id);
    assert_eq!(outcome.reason, TimerFinishReason::Completed);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn timer_events_are_consumable_from_the_public_api() {
    let timer = Timer::new();
    let mut events = timer.subscribe();
    let run_id = timer
        .start_once(Duration::from_secs(1), || async { Ok(()) })
        .await
        .unwrap();

    assert!(matches!(
        events.wait_started().await,
        Some(TimerEvent::Started { run_id: seen, .. }) if seen == run_id
    ));

    advance(Duration::from_secs(1)).await;
    settle().await;

    assert!(matches!(
        events.wait_tick().await,
        Some(TimerEvent::Tick { run_id: seen, .. }) if seen == run_id
    ));
    assert!(events.wait_finished().await.is_some());
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn registry_spawn_helpers_reduce_boilerplate() {
    let registry = TimerRegistry::new();
    let (timer_id, timer) = registry
        .start_once(Duration::from_secs(2), || async { Ok(()) })
        .await
        .unwrap();

    assert!(registry.get(timer_id).await.is_some());

    advance(Duration::from_secs(2)).await;
    settle().await;

    let outcome = timer.join().await.unwrap();
    assert_eq!(outcome.reason, TimerFinishReason::Completed);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn builder_api_is_simple_to_use() {
    let timer = Timer::once(Duration::from_secs(3))
        .start(|| async { Ok(()) })
        .await
        .unwrap();

    advance(Duration::from_secs(3)).await;
    settle().await;

    let outcome = timer.join().await.unwrap();
    assert_eq!(outcome.reason, TimerFinishReason::Completed);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn completion_api_is_simple_to_consume() {
    let timer = Timer::new();
    let mut completion = timer.completion();
    let run_id = timer
        .start_once(Duration::from_secs(1), || async { Ok(()) })
        .await
        .unwrap();

    advance(Duration::from_secs(1)).await;
    settle().await;

    let outcome = completion.wait_for_run(run_id).await.unwrap();
    assert_eq!(outcome.run_id, run_id);
    assert_eq!(outcome.reason, TimerFinishReason::Completed);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn registry_ergonomics_cover_common_bulk_operations() {
    let registry = TimerRegistry::new();
    let (first_id, _first_timer) = registry
        .start_once(Duration::from_secs(1), || async { Ok(()) })
        .await
        .unwrap();
    let (second_id, second_timer) = registry
        .start_once(Duration::from_secs(2), || async { Ok(()) })
        .await
        .unwrap();

    assert!(registry.contains(first_id).await);
    assert!(registry.stop(first_id).await.unwrap().is_some());
    assert!(registry.cancel(999_999).await.unwrap().is_none());

    advance(Duration::from_secs(2)).await;
    settle().await;

    let joined = registry.join_all().await;
    assert!(joined.iter().any(|(id, outcome)| {
        *id == second_id && outcome.reason == TimerFinishReason::Completed
    }));

    assert_eq!(
        second_timer.join().await.unwrap().reason,
        TimerFinishReason::Completed
    );
    assert_eq!(registry.clear().await, 2);
    assert!(registry.is_empty().await);
}
