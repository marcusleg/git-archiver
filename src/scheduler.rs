//! The scheduler: fetches one tracked repo every `interval ÷ N`, the one with
//! the oldest last attempt first, one at a time. Repos added in the UI jump
//! the queue.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::archive::{Archives, FetchError};
use crate::clock::Clock;
use crate::config::Tokens;
use crate::overview::{SharedOverview, Tracked};
use crate::tracking::Tracking;

/// Every fetch is stopped after this long.
const FETCH_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// How often the config is re-read while waiting for the next fetch, so
/// hand-added or removed repos change the gap without waiting a whole gap.
const CONFIG_RECHECK: Duration = Duration::from_secs(60);

pub struct Scheduler {
    pub clock: Arc<dyn Clock>,
    pub archives: Arc<Archives>,
    pub interval: Duration,
    pub started: SystemTime,
    pub overview: SharedOverview,
    pub tracking: Arc<Tracking>,
    /// From the last good config, so a changed token counts from the next
    /// fetch.
    pub tokens: Tokens,
}

impl Scheduler {
    pub async fn run(mut self) {
        loop {
            self.reload_tracked();
            let now = self.clock.now();
            match self.next_fetch(now) {
                Some((due, repo)) if due <= now => self.fetch(repo).await,
                Some((due, _)) => self.wait_until(due.min(now + CONFIG_RECHECK)).await,
                None => self.wait_until(now + CONFIG_RECHECK).await,
            }
        }
    }

    /// Waits until `deadline`, or until woken because a repo jumped the
    /// queue.
    async fn wait_until(&self, deadline: SystemTime) {
        tokio::select! {
            () = self.clock.sleep_until(deadline) => {}
            () = self.tracking.changed() => {}
        }
    }

    /// Re-reads the repo list and tokens. An unreadable or invalid config
    /// keeps the last good ones, and the problem shows on the list page until
    /// the file is valid again. Queued repos no longer tracked leave the queue.
    fn reload_tracked(&mut self) {
        match self.tracking.reload() {
            Ok(tokens) => {
                self.tokens = tokens;
                if self.overview.set_config_error(None) {
                    tracing::info!("config is valid again");
                }
            }
            Err(error) => {
                // Logged once per distinct error, so a broken file isn't
                // logged on every reload.
                if self.overview.set_config_error(Some(error.clone())) {
                    tracing::error!(error, "keeping the last good config");
                }
            }
        }
    }

    /// When the next fetch is due and which repo it fetches.
    fn next_fetch(&self, now: SystemTime) -> Option<(SystemTime, Tracked)> {
        self.overview.next_fetch(self.interval, self.started, now)
    }

    /// Fetches the repo and records the result, unless the repo is untracked
    /// before the fetch finishes.
    async fn fetch(&self, repo: Tracked) {
        // Held until the state is saved, so deleting an archive never
        // interleaves with fetching it.
        let _exclusive = self.archives.exclusive(&repo.id).await;
        let had_archive = self.archives.exists(&repo.id);
        let attempt = whole_seconds(self.clock.now());
        let timer = Instant::now();
        let token = self.tokens.for_repo(&repo.id);
        let result = tokio::select! {
            biased;
            () = self.tracking.until_untracked(&repo.id) => {
                tracing::info!(repo = %repo.id, "stopped fetching: no longer tracked");
                // A first fetch stopped midway leaves an empty archive,
                // which isn't worth keeping. Open pages may already have
                // listed it as untracked, so its deletion is announced too.
                if !had_archive {
                    match self.archives.delete(&repo.id).await {
                        Ok(()) => self.overview.forget(&repo.id),
                        Err(e) => {
                            tracing::warn!(repo = %repo.id, error = %format!("{e:#}"), "couldn't delete archive");
                        }
                    }
                }
                return;
            }
            result = async {
                tracing::info!(repo = %repo.id, "fetching");
                self.archives.fetch(&repo.id, &repo.url, attempt, token).await
            } => result,
            () = self.clock.sleep_until(attempt + FETCH_TIMEOUT) => Err(FetchError::Failed {
                reason: format!(
                    "fetch timed out after {}",
                    humantime::format_duration(FETCH_TIMEOUT)
                ),
            }),
        };
        let duration = timer.elapsed();
        let size = self.archives.size(&repo.id).await;

        self.overview
            .record_fetch(&repo.id, attempt, duration, size, result);
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
