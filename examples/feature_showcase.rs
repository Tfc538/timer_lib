use async_trait::async_trait;
use std::time::Duration;
use timer_lib::{Timer, TimerCallback, TimerManager};
use tokio::time::sleep;

struct OneTimeCallback;
struct RecurringCallback;
struct ErrorCallback;

#[async_trait]
impl TimerCallback for OneTimeCallback {
    async fn execute(&self) -> Result<(), timer_lib::TimerError> {
        println!("One-time timer executed!");
        Ok(())
    }
}

#[async_trait]
impl TimerCallback for RecurringCallback {
    async fn execute(&self) -> Result<(), timer_lib::TimerError> {
        println!("Recurring timer executed!");
        Ok(())
    }
}

#[async_trait]
impl TimerCallback for ErrorCallback {
    async fn execute(&self) -> Result<(), timer_lib::TimerError> {
        Err(timer_lib::TimerError::CallbackError(
            "Simulated error!".into(),
        ))
    }
}

#[tokio::main]
async fn main() {
    let manager = TimerManager::new();

    // 1. One-time Timer
    let mut one_time_timer = Timer::new();
    one_time_timer
        .start_once(Duration::from_secs(2), OneTimeCallback)
        .await
        .unwrap();
    manager.add_timer(one_time_timer);

    // 2. Recurring Timer
    let mut recurring_timer = Timer::new();
    recurring_timer
        .start_recurring(Duration::from_secs(3), RecurringCallback, Some(5))
        .await
        .unwrap();
    let recurring_timer_id = manager.add_timer(recurring_timer);

    // 3. Pause and Resume
    sleep(Duration::from_secs(6)).await;
    println!("Pausing recurring timer...");
    if let Some(timer) = manager
        .get_timer(recurring_timer_id)
        .and_then(|t| t.lock().ok().map(|t| t.clone()))
    {
        timer.pause().await.unwrap();
    }

    sleep(Duration::from_secs(3)).await; // Wait while paused
    println!("Resuming recurring timer...");
    if let Some(timer) = manager
        .get_timer(recurring_timer_id)
        .and_then(|t| t.lock().ok().map(|t| t.clone()))
    {
        timer.resume().await.unwrap();
    }

    // 4. Dynamic Interval Adjustment
    sleep(Duration::from_secs(6)).await;
    println!("Adjusting recurring timer interval...");
    if let Some(mut timer) = manager.get_timer(recurring_timer_id).and_then(|t: std::sync::Arc<std::sync::Mutex<Timer>>| t.lock().ok().map(|t| t.clone())) {
        timer.adjust_interval(Duration::from_secs(1)).unwrap();
    }

    // 5. Timer Statistics
    sleep(Duration::from_secs(10)).await;
    println!("Retrieving timer statistics...");
    if let Some(timer) = manager.get_timer(recurring_timer_id).and_then(|t| t.lock().ok().map(|t| t.clone())) {
        let stats = timer.get_statistics().await;
        println!("Timer statistics: {:?}", stats);
    }

    // 6. Error Handling
    println!("Starting a timer with an error callback...");
    let mut error_timer = Timer::new();
    if let Err(e) = error_timer
        .start_once(Duration::from_secs(2), ErrorCallback)
        .await
    {
        println!("Error while starting timer: {}", e);
    }

    println!("All timers completed!");
}
