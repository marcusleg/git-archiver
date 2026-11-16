//! A clock that only moves when the test advances it.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use git_archiver::clock::{Clock, Sleep};
use tokio::sync::watch;

/// When every test's clock starts: 2026-11-16T12:00:00Z.
pub fn start_time() -> SystemTime {
    humantime::parse_rfc3339("2026-11-16T12:00:00Z").unwrap()
}

pub struct ManualClock {
    now: watch::Sender<SystemTime>,
}

impl ManualClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            now: watch::Sender::new(start_time()),
        })
    }

    pub fn advance(&self, by: Duration) {
        self.now.send_modify(|now| *now += by);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> SystemTime {
        *self.now.borrow()
    }

    fn sleep_until(&self, deadline: SystemTime) -> Sleep {
        let mut now = self.now.subscribe();
        Box::pin(async move {
            while *now.borrow_and_update() < deadline {
                if now.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        })
    }
}
