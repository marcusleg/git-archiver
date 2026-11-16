//! The clock seam: the app reads wall-clock time and waits for deadlines only
//! through a [`Clock`], so tests can set and advance time.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, SystemTime};

pub type Sleep = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Wall-clock time for scheduling, timeouts and the times shown to people.
pub trait Clock: Send + Sync + 'static {
    fn now(&self) -> SystemTime;

    /// Resolves once `now()` has reached `deadline`.
    fn sleep_until(&self, deadline: SystemTime) -> Sleep;
}

/// The real clock.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn sleep_until(&self, deadline: SystemTime) -> Sleep {
        Box::pin(async move {
            // Re-checks after every sleep, because the wall clock can be set
            // back while we wait.
            while let Ok(left) = deadline.duration_since(SystemTime::now()) {
                if left.is_zero() {
                    break;
                }
                tokio::time::sleep(left.min(Duration::from_secs(60))).await;
            }
        })
    }
}
