use async_trait::async_trait;
use std::time::Duration;
use timer_lib::{RecurringSchedule, Timer, TimerCallback, TimerError, TimerRegistry};
use tokio::time::sleep;

struct OneTimeCallback;
struct RecurringCallback;
struct ErrorCallback;

#[async_trait]
impl TimerCallback for OneTimeCallback {
    async fn execute(&self) -> Result<(), TimerError> {
        println!("One-time timer executed");
        Ok(())
    }
}

#[async_trait]
impl TimerCallback for RecurringCallback {
    async fn execute(&self) -> Result<(), TimerError> {
        println!("Recurring timer executed");
        Ok(())
    }
}

#[async_trait]
impl TimerCallback for ErrorCallback {
    async fn execute(&self) -> Result<(), TimerError> {
        Err(TimerError::callback_failed("Simulated error"))
    }
}

#[tokio::main]
async fn main() {
    let registry = TimerRegistry::new();

    let one_time_timer = Timer::once(Duration::from_secs(2))
        .start(OneTimeCallback)
        .await
        .unwrap();
    registry.insert(one_time_timer.clone()).await;

    let recurring_timer =
        Timer::recurring(RecurringSchedule::new(Duration::from_secs(3)).with_expiration_count(5))
            .start(RecurringCallback)
            .await
            .unwrap();
    let recurring_timer_id = registry.insert(recurring_timer.clone()).await;
    let mut recurring_events = recurring_timer.subscribe();
    let mut recurring_completion = recurring_timer.completion();

    sleep(Duration::from_secs(4)).await;
    println!("Pausing recurring timer");
    recurring_timer.pause().await.unwrap();

    sleep(Duration::from_secs(3)).await;
    println!("Updating interval while paused");
    recurring_timer
        .adjust_interval(Duration::from_secs(1))
        .await
        .unwrap();
    println!("Resuming recurring timer");
    recurring_timer.resume().await.unwrap();

    while let Some(event) = recurring_events.recv().await {
        println!("Recurring event: {:?}", event);
        if matches!(event, timer_lib::TimerEvent::Finished(_)) {
            break;
        }
    }

    let outcome = recurring_completion.wait().await.unwrap();
    let stats = recurring_timer.get_statistics().await;
    println!("Recurring timer outcome: {:?}", outcome);
    println!("Recurring timer statistics: {:?}", stats);
    println!(
        "Registry still tracks active timer ids: {:?}",
        registry.active_ids().await
    );

    let error_timer = Timer::new();
    error_timer
        .start_once(Duration::from_secs(1), ErrorCallback)
        .await
        .unwrap();
    let error_outcome = error_timer.join().await.unwrap();
    println!("Error timer outcome: {:?}", error_outcome);
    println!(
        "Last callback error: {:?}",
        error_timer.get_last_error().await
    );

    assert!(registry.get(recurring_timer_id).await.is_some());
    registry.stop_all().await;
}
