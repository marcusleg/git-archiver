//! The scheduler: fetches one tracked repo every `interval ÷ N`, the one with
//! the oldest last attempt first, one at a time.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::archive::Archives;
use crate::clock::Clock;
use crate::config;
use crate::identity::RepoId;
use crate::state::{State, Status};

/// Every fetch is stopped after this long.
const FETCH_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// How often the config is checked for repos while none are tracked.
const IDLE_RECHECK: Duration = Duration::from_secs(60);

/// What the scheduler knows, shared with the web UI and `/metrics`.
#[derive(Debug, Default)]
pub struct Snapshot {
    /// In config order, one entry per repo identity.
    pub tracked: Vec<Tracked>,
    pub state: State,
    pub next_fetch: Option<SystemTime>,
    pub last_fetch_duration: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct Tracked {
    /// As written in the config.
    pub url: String,
    pub id: RepoId,
}

pub type Shared = Arc<Mutex<Snapshot>>;

/// The tracked repos for a config's repo list: the first URL for each
/// identity, in config order.
pub fn tracked(urls: &[String]) -> Vec<Tracked> {
    let mut tracked: Vec<Tracked> = Vec::new();
    for url in urls {
        let Ok(id) = RepoId::from_url(url) else {
            continue;
        };
        if !tracked.iter().any(|t| t.id == id) {
            tracked.push(Tracked {
                url: url.clone(),
                id,
            });
        }
    }
    tracked
}

pub struct Scheduler {
    pub clock: Arc<dyn Clock>,
    pub archives: Arc<Archives>,
    pub config_path: PathBuf,
    pub state_path: PathBuf,
    pub interval: Duration,
    pub started: SystemTime,
    pub shared: Shared,
}

impl Scheduler {
    pub async fn run(self) {
        loop {
            self.reload_tracked();
            let now = self.clock.now();
            let Some((due, repo)) = self.next_fetch() else {
                self.shared.lock().unwrap().next_fetch = None;
                self.clock.sleep_until(now + IDLE_RECHECK).await;
                continue;
            };
            self.shared.lock().unwrap().next_fetch = Some(due);
            if now < due {
                self.clock.sleep_until(due).await;
                // Wakes to re-read the config: the due time may have moved.
                continue;
            }
            self.fetch(repo).await;
        }
    }

    /// Re-reads the repo list. An unreadable or invalid config keeps the
    /// last good list.
    fn reload_tracked(&self) {
        match config::load(&self.config_path) {
            Ok(config) => self.shared.lock().unwrap().tracked = tracked(&config.repos),
            Err(e) => tracing::error!(error = %format!("{e:#}"), "keeping the last good config"),
        }
    }

    /// When the next fetch is due and which repo it fetches.
    fn next_fetch(&self) -> Option<(SystemTime, Tracked)> {
        let snapshot = self.shared.lock().unwrap();
        let count = u32::try_from(snapshot.tracked.len()).unwrap_or(u32::MAX);
        if count == 0 {
            return None;
        }
        let gap = self.interval / count;
        let last_attempt = |t: &Tracked| {
            snapshot
                .state
                .repos
                .get(t.id.as_str())
                .and_then(|s| s.last_attempt)
        };
        let latest = snapshot.tracked.iter().filter_map(last_attempt).max();
        let due = latest.unwrap_or(self.started) + gap;
        // Never-attempted repos sort first (`None < Some`); `min_by_key`
        // keeps the first of equals, so ties go by config order.
        let repo = snapshot.tracked.iter().min_by_key(|t| last_attempt(t))?;
        Some((due, repo.clone()))
    }

    async fn fetch(&self, repo: Tracked) {
        let attempt = whole_seconds(self.clock.now());
        let timer = Instant::now();
        tracing::info!(repo = %repo.id, "fetching");
        let result = tokio::select! {
            result = self.archives.fetch(&repo.id, &repo.url) => result,
            () = self.clock.sleep_until(attempt + FETCH_TIMEOUT) => {
                Err(anyhow::anyhow!("fetch timed out after {}", humantime::format_duration(FETCH_TIMEOUT)))
            }
        };
        let duration = timer.elapsed();
        let size = self.archives.size(&repo.id).await;

        let mut snapshot = self.shared.lock().unwrap();
        snapshot.last_fetch_duration = Some(duration);
        let entry = snapshot.state.repos.entry(repo.id.to_string()).or_default();
        entry.last_attempt = Some(attempt);
        entry.size_bytes = size;
        match result {
            Ok(()) => {
                entry.last_success = Some(attempt);
                if entry.status != Some(Status::Available) {
                    entry.status = Some(Status::Available);
                    entry.status_since = Some(attempt);
                    tracing::info!(repo = %repo.id, status = "available", "status changed");
                }
                tracing::info!(repo = %repo.id, duration_ms = duration.as_millis(), "fetched");
            }
            Err(e) => {
                tracing::warn!(repo = %repo.id, error = %format!("{e:#}"), "fetch failed");
            }
        }
        let state = snapshot.state.clone();
        drop(snapshot);
        if let Err(e) = state.save(&self.state_path) {
            tracing::error!(error = %format!("{e:#}"), "couldn't save state");
        }
    }
}

/// State records times to the second, so in-memory times match saved ones.
fn whole_seconds(time: SystemTime) -> SystemTime {
    let secs = time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    UNIX_EPOCH + Duration::from_secs(secs)
}
