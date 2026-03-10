use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

use tokio::sync::RwLock;

use crate::errors::TimerError;
use crate::timer::{Timer, TimerCallback, TimerOutcome, TimerState};

/// A registry for tracking timers by identifier.
#[derive(Clone, Default)]
pub struct TimerRegistry {
    timers: Arc<RwLock<HashMap<u64, Timer>>>,
    next_id: Arc<AtomicU64>,
}

impl TimerRegistry {
    /// Creates a new timer registry.
    pub fn new() -> Self {
        Self {
            timers: Arc::new(RwLock::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Inserts an existing timer and returns its identifier.
    pub async fn insert(&self, timer: Timer) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.timers.write().await.insert(id, timer);
        id
    }

    /// Starts and registers a one-time timer.
    pub async fn start_once<F>(
        &self,
        delay: Duration,
        callback: F,
    ) -> Result<(u64, Timer), TimerError>
    where
        F: TimerCallback + 'static,
    {
        let timer = Timer::new();
        let _ = timer.start_once(delay, callback).await?;
        let id = self.insert(timer.clone()).await;
        Ok((id, timer))
    }

    /// Starts and registers a recurring timer.
    pub async fn start_recurring<F>(
        &self,
        interval: Duration,
        callback: F,
        expiration_count: Option<usize>,
    ) -> Result<(u64, Timer), TimerError>
    where
        F: TimerCallback + 'static,
    {
        let timer = Timer::new();
        let _ = timer
            .start_recurring(interval, callback, expiration_count)
            .await?;
        let id = self.insert(timer.clone()).await;
        Ok((id, timer))
    }

    /// Removes a timer from the registry and returns it.
    pub async fn remove(&self, id: u64) -> Option<Timer> {
        self.timers.write().await.remove(&id)
    }

    /// Returns true when the registry tracks the given timer identifier.
    pub async fn contains(&self, id: u64) -> bool {
        self.timers.read().await.contains_key(&id)
    }

    /// Stops a timer by identifier when it exists.
    pub async fn stop(&self, id: u64) -> Result<Option<TimerOutcome>, TimerError> {
        let timer = self.get(id).await;
        match timer {
            Some(timer) => timer.stop().await.map(Some),
            None => Ok(None),
        }
    }

    /// Cancels a timer by identifier when it exists.
    pub async fn cancel(&self, id: u64) -> Result<Option<TimerOutcome>, TimerError> {
        let timer = self.get(id).await;
        match timer {
            Some(timer) => timer.cancel().await.map(Some),
            None => Ok(None),
        }
    }

    /// Pauses a timer by identifier when it exists.
    pub async fn pause(&self, id: u64) -> Result<bool, TimerError> {
        let timer = self.get(id).await;
        match timer {
            Some(timer) => {
                timer.pause().await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Resumes a timer by identifier when it exists.
    pub async fn resume(&self, id: u64) -> Result<bool, TimerError> {
        let timer = self.get(id).await;
        match timer {
            Some(timer) => {
                timer.resume().await?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Stops all timers currently tracked by the registry.
    pub async fn stop_all(&self) {
        let timers: Vec<Timer> = self.timers.read().await.values().cloned().collect();
        for timer in timers {
            let _ = timer.stop().await;
        }
    }

    /// Pauses all running timers currently tracked by the registry.
    pub async fn pause_all(&self) {
        let timers: Vec<Timer> = self.timers.read().await.values().cloned().collect();
        for timer in timers {
            let _ = timer.pause().await;
        }
    }

    /// Waits for all tracked timers that have a joinable outcome.
    pub async fn join_all(&self) -> Vec<(u64, TimerOutcome)> {
        let timers: Vec<(u64, Timer)> = self
            .timers
            .read()
            .await
            .iter()
            .map(|(id, timer)| (*id, timer.clone()))
            .collect();

        let mut outcomes = Vec::with_capacity(timers.len());
        for (id, timer) in timers {
            if let Ok(outcome) = timer.join().await {
                outcomes.push((id, outcome));
            }
        }

        outcomes
    }

    /// Cancels all timers currently tracked by the registry.
    pub async fn cancel_all(&self) {
        let timers: Vec<Timer> = self.timers.read().await.values().cloned().collect();
        for timer in timers {
            let _ = timer.cancel().await;
        }
    }

    /// Resumes all paused timers currently tracked by the registry.
    pub async fn resume_all(&self) {
        let timers: Vec<Timer> = self.timers.read().await.values().cloned().collect();
        for timer in timers {
            let _ = timer.resume().await;
        }
    }

    /// Lists all active timers.
    pub async fn active_ids(&self) -> Vec<u64> {
        let timers: Vec<(u64, Timer)> = self
            .timers
            .read()
            .await
            .iter()
            .map(|(id, timer)| (*id, timer.clone()))
            .collect();

        let mut active = Vec::new();
        for (id, timer) in timers {
            if timer.get_state().await != TimerState::Stopped {
                active.push(id);
            }
        }
        active
    }

    /// Retrieves a timer by ID.
    pub async fn get(&self, id: u64) -> Option<Timer> {
        self.timers.read().await.get(&id).cloned()
    }

    /// Returns the number of tracked timers.
    pub async fn len(&self) -> usize {
        self.timers.read().await.len()
    }

    /// Returns true when the registry is empty.
    pub async fn is_empty(&self) -> bool {
        self.len().await == 0
    }

    /// Removes all tracked timers and returns the number removed.
    pub async fn clear(&self) -> usize {
        let mut timers = self.timers.write().await;
        let removed = timers.len();
        timers.clear();
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timer::TimerFinishReason;
    use tokio::task::yield_now;
    use tokio::time::advance;

    async fn settle() {
        for _ in 0..5 {
            yield_now().await;
        }
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn registry_start_helpers_are_easy_to_use() {
        let registry = TimerRegistry::new();
        let (once_id, once_timer) = registry
            .start_once(Duration::from_secs(1), || async { Ok(()) })
            .await
            .unwrap();
        let (recurring_id, recurring_timer) = registry
            .start_recurring(Duration::from_secs(2), || async { Ok(()) }, None)
            .await
            .unwrap();

        assert_ne!(once_id, recurring_id);
        assert_eq!(registry.len().await, 2);
        assert!(registry.get(once_id).await.is_some());

        advance(Duration::from_secs(1)).await;
        settle().await;
        assert_eq!(
            once_timer.join().await.unwrap().reason,
            crate::timer::TimerFinishReason::Completed
        );

        let active = registry.active_ids().await;
        assert!(active.contains(&recurring_id));

        let _ = recurring_timer.cancel().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn registry_supports_direct_timer_controls() {
        let registry = TimerRegistry::new();
        let (timer_id, _timer) = registry
            .start_once(Duration::from_secs(5), || async { Ok(()) })
            .await
            .unwrap();

        assert!(registry.contains(timer_id).await);
        let outcome = registry.cancel(timer_id).await.unwrap().unwrap();
        assert_eq!(outcome.reason, TimerFinishReason::Cancelled);
        assert_eq!(registry.clear().await, 1);
        assert!(registry.is_empty().await);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn registry_can_pause_and_resume_tracked_timers() {
        let registry = TimerRegistry::new();
        let (timer_id, timer) = registry
            .start_recurring(Duration::from_secs(2), || async { Ok(()) }, Some(1))
            .await
            .unwrap();
        settle().await;

        assert!(registry.pause(timer_id).await.unwrap());
        assert_eq!(timer.get_state().await, TimerState::Paused);

        advance(Duration::from_secs(5)).await;
        settle().await;
        assert_eq!(timer.get_statistics().await.execution_count, 0);

        assert!(registry.resume(timer_id).await.unwrap());
        advance(Duration::from_secs(2)).await;
        settle().await;
        assert_eq!(
            timer.join().await.unwrap().reason,
            TimerFinishReason::Completed
        );
    }
}
