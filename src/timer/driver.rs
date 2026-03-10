use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use tokio::task::JoinHandle;
use tokio::time::{self, Instant};

#[cfg(feature = "test-util")]
use std::collections::VecDeque;
#[cfg(feature = "test-util")]
use std::sync::{Arc, Mutex};
#[cfg(feature = "test-util")]
use tokio::sync::watch;

pub(crate) type SleepFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

#[derive(Clone, Default)]
pub(crate) enum RuntimeHandle {
    #[default]
    Native,

    #[cfg(feature = "test-util")]
    Mock(Arc<MockRuntimeInner>),
}

impl RuntimeHandle {
    pub(super) fn now(&self) -> Instant {
        match self {
            Self::Native => Instant::now(),

            #[cfg(feature = "test-util")]
            Self::Mock(inner) => inner.now(),
        }
    }

    pub(super) fn sleep(&self, duration: Duration) -> SleepFuture {
        self.sleep_until(self.now() + duration)
    }

    pub(super) fn sleep_until(&self, deadline: Instant) -> SleepFuture {
        match self {
            Self::Native => Box::pin(time::sleep_until(deadline)),

            #[cfg(feature = "test-util")]
            Self::Mock(inner) => inner.sleep_until(deadline),
        }
    }

    pub(super) fn sample_jitter(&self, max_jitter: Duration) -> Duration {
        if max_jitter.is_zero() {
            return Duration::ZERO;
        }

        match self {
            Self::Native => {
                let jitter_nanos = max_jitter.as_nanos().min(u64::MAX as u128) as u64;
                Duration::from_nanos(fastrand::u64(0..=jitter_nanos))
            }

            #[cfg(feature = "test-util")]
            Self::Mock(inner) => inner.sample_jitter(max_jitter),
        }
    }

    pub(super) fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        tokio::spawn(future)
    }
}

#[cfg(feature = "test-util")]
#[derive(Clone)]
pub struct MockRuntime {
    inner: Arc<MockRuntimeInner>,
}

#[cfg(feature = "test-util")]
impl Default for MockRuntime {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "test-util")]
impl MockRuntime {
    /// Creates a new manually-driven runtime clock for tests.
    pub fn new() -> Self {
        let now = Instant::now();
        let (time_tx, _time_rx) = watch::channel(now);
        Self {
            inner: Arc::new(MockRuntimeInner {
                now: Mutex::new(now),
                time_tx,
                jitter_samples: Mutex::new(VecDeque::new()),
            }),
        }
    }

    pub(crate) fn handle(&self) -> RuntimeHandle {
        RuntimeHandle::Mock(Arc::clone(&self.inner))
    }

    /// Advances the mocked clock and wakes waiting timers.
    pub async fn advance(&self, duration: Duration) {
        self.inner.advance(duration);
        self.settle().await;
    }

    /// Yields long enough for spawned timer tasks to observe the latest clock state.
    pub async fn settle(&self) {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    /// Queues the next jitter sample to use for recurring schedules.
    pub fn push_jitter(&self, jitter: Duration) {
        self.inner.push_jitter(jitter);
    }

    /// Returns the mocked current time.
    pub fn now(&self) -> Instant {
        self.inner.now()
    }
}

#[cfg(feature = "test-util")]
pub(crate) struct MockRuntimeInner {
    now: Mutex<Instant>,
    time_tx: watch::Sender<Instant>,
    jitter_samples: Mutex<VecDeque<Duration>>,
}

#[cfg(feature = "test-util")]
impl MockRuntimeInner {
    fn now(&self) -> Instant {
        *self.now.lock().expect("mock runtime now lock poisoned")
    }

    fn advance(&self, duration: Duration) {
        let mut now = self.now.lock().expect("mock runtime now lock poisoned");
        *now += duration;
        self.time_tx.send_replace(*now);
    }

    fn push_jitter(&self, jitter: Duration) {
        self.jitter_samples
            .lock()
            .expect("mock runtime jitter lock poisoned")
            .push_back(jitter);
    }

    fn sample_jitter(&self, max_jitter: Duration) -> Duration {
        self.jitter_samples
            .lock()
            .expect("mock runtime jitter lock poisoned")
            .pop_front()
            .unwrap_or(Duration::ZERO)
            .min(max_jitter)
    }

    fn sleep_until(&self, deadline: Instant) -> SleepFuture {
        let mut rx = self.time_tx.subscribe();
        Box::pin(async move {
            loop {
                if *rx.borrow_and_update() >= deadline {
                    return;
                }

                if rx.changed().await.is_err() {
                    return;
                }
            }
        })
    }
}
