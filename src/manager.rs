use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::timer::Timer;

/// A manager for controlling multiple timers.
pub struct TimerManager {
    timers: Arc<Mutex<HashMap<u64, Timer>>>,
    next_id: Arc<Mutex<u64>>,
}

impl Default for TimerManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TimerManager {
    /// Creates a new timer manager.
    pub fn new() -> Self {
        TimerManager {
            timers: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(0)),
        }
    }

    /// Adds a timer to the manager and returns its ID.
    pub fn add_timer(&self, timer: Timer) -> u64 {
        let mut timers = self.timers.lock().unwrap();
        let mut next_id = self.next_id.lock().unwrap();
        let id = *next_id;
        *next_id += 1;

        timers.insert(id, timer);
        id
    }

    /// Stops all timers.
    pub fn stop_all(&self) {
        let mut timers = self.timers.lock().unwrap();
        for timer in timers.values_mut() {
            let _ = timer.stop();
        }
    }

    /// Lists all active timers.
    pub fn list_timers(&self) -> Vec<u64> {
        self.timers
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(id, timer)| {
                if futures::executor::block_on(timer.get_state()) != crate::timer::TimerState::Stopped {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Retrieves a timer by ID.
    pub fn get_timer(&self, id: u64) -> Option<Arc<Mutex<Timer>>> {
        self.timers.lock().unwrap().get(&id).cloned().map(|timer| Arc::new(Mutex::new(timer)))
    }
}

unsafe impl Send for TimerManager {}
unsafe impl Sync for TimerManager {}
