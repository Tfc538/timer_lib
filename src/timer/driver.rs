use std::future::Future;

use tokio::task::JoinHandle;
use tokio::time::{self, Instant, Sleep};

#[derive(Clone, Copy, Default)]
pub(crate) struct RuntimeHandle;

impl RuntimeHandle {
    pub(super) fn now(self) -> Instant {
        Instant::now()
    }

    pub(super) fn sleep(self, duration: std::time::Duration) -> Sleep {
        time::sleep(duration)
    }

    pub(super) fn spawn<F>(self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        tokio::spawn(future)
    }
}
