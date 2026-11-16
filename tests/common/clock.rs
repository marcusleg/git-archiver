//! A clock that only moves when the test advances it, and knows who is
//! waiting on it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use git_archiver::clock::{Clock, Sleep};
use tokio::sync::watch;

/// When every test's clock starts: 2026-11-16T12:00:00Z.
pub fn start_time() -> SystemTime {
    humantime::parse_rfc3339("2026-11-16T12:00:00Z").unwrap()
}

pub struct ManualClock {
    now: watch::Sender<SystemTime>,
    /// Deadlines of the sleeps currently waited on, by sleep.
    sleepers: Arc<Mutex<HashMap<u64, SystemTime>>>,
    next_sleeper: AtomicU64,
}

impl ManualClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            now: watch::Sender::new(start_time()),
            sleepers: Arc::default(),
            next_sleeper: AtomicU64::new(0),
        })
    }

    pub fn advance(&self, by: Duration) {
        self.now.send_modify(|now| *now += by);
    }

    /// True when the app is waiting on the clock and nothing it waits on is
    /// due yet: it has done everything it can at the current time, or is
    /// waiting for a timeout.
    pub fn idle(&self) -> bool {
        let now = self.now();
        let sleepers = self.sleepers.lock().unwrap();
        !sleepers.is_empty() && sleepers.values().all(|deadline| *deadline > now)
    }
}

/// Removes a sleep from the waiting list when it finishes or is dropped.
struct Sleeper {
    id: u64,
    sleepers: Arc<Mutex<HashMap<u64, SystemTime>>>,
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        self.sleepers.lock().unwrap().remove(&self.id);
    }
}

impl Clock for ManualClock {
    fn now(&self) -> SystemTime {
        *self.now.borrow()
    }

    fn sleep_until(&self, deadline: SystemTime) -> Sleep {
        let mut now = self.now.subscribe();
        let id = self.next_sleeper.fetch_add(1, Ordering::Relaxed);
        self.sleepers.lock().unwrap().insert(id, deadline);
        let sleeper = Sleeper {
            id,
            sleepers: self.sleepers.clone(),
        };
        Box::pin(async move {
            let _sleeper = sleeper;
            while *now.borrow_and_update() < deadline {
                if now.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        })
    }
}
