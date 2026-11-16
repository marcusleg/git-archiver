//! The scheduler: fetches one tracked repo every `interval ÷ N`, the one with
//! the oldest last attempt first, one at a time. Repos added in the UI jump
//! the queue.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::sync::Notify;

use crate::archive::{Archives, FetchError};
use crate::clock::Clock;
use crate::config::{self, Tokens};
use crate::identity::RepoId;
use crate::overview::{Overview, SharedOverview, Tracked};
use crate::state::Status;

/// Every fetch is stopped after this long.
const FETCH_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// How often the config is re-read while waiting for the next fetch, so
/// hand-added or removed repos change the gap without waiting a whole gap.
const CONFIG_RECHECK: Duration = Duration::from_secs(60);

pub struct Scheduler {
    pub clock: Arc<dyn Clock>,
    pub archives: Arc<Archives>,
    pub config_path: PathBuf,
    pub state_path: PathBuf,
    pub interval: Duration,
    pub started: SystemTime,
    pub overview: SharedOverview,
    /// Wakes the scheduler from waiting, after a repo jumped the queue.
    pub wake: Arc<Notify>,
    /// From the last good config, so a changed token counts from the next
    /// fetch.
    pub tokens: Tokens,
    /// Notified after a repo was untracked in the UI, to stop its fetch.
    pub untracked: Arc<Notify>,
    /// The last config error logged, so a broken file is logged once.
    pub config_error: Option<String>,
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
            () = self.wake.notified() => {}
        }
    }

    /// Re-reads the repo list and tokens. An unreadable or invalid config
    /// keeps the last good ones, and the problem shows on the list page until
    /// the file is valid again. Queued repos no longer tracked leave the queue.
    fn reload_tracked(&mut self) {
        let overview = &self.overview;
        let result = config::load_locked(&self.config_path, |config| {
            let config = config.map_err(|e| format!("{e:#}"))?;
            overview.lock().unwrap().set_tracked(&config.repos);
            Ok(config.tokens)
        });
        match result {
            Ok(tokens) => {
                self.tokens = tokens;
                if self.config_error.take().is_some() {
                    tracing::info!("config is valid again");
                    self.overview.lock().unwrap().config_error = None;
                }
            }
            Err(error) => {
                if self.config_error.as_ref() != Some(&error) {
                    tracing::error!(error, "keeping the last good config");
                    self.overview.lock().unwrap().config_error = Some(error.clone());
                    self.config_error = Some(error);
                }
            }
        }
    }

    /// When the next fetch is due and which repo it fetches, also recorded
    /// in the overview. A repo that jumped the queue is due now.
    fn next_fetch(&self, now: SystemTime) -> Option<(SystemTime, Tracked)> {
        // One lock for reading the queue and recording the result, so a repo
        // jumping the queue meanwhile isn't hidden by a stale due time.
        let mut overview = self.overview.lock().unwrap();
        let next = Self::next_fetch_in(&overview, self.interval, self.started, now);
        overview.next_fetch = next.as_ref().map(|(due, _)| *due);
        next
    }

    fn next_fetch_in(
        overview: &Overview,
        interval: Duration,
        started: SystemTime,
        now: SystemTime,
    ) -> Option<(SystemTime, Tracked)> {
        let first = overview
            .fetch_first
            .iter()
            .find_map(|id| overview.tracked.iter().find(|t| &t.id == id));
        if let Some(repo) = first {
            return Some((now, repo.clone()));
        }
        let count = u32::try_from(overview.tracked.len()).unwrap_or(u32::MAX);
        if count == 0 {
            return None;
        }
        let gap = interval / count;
        let last_attempt = |t: &Tracked| overview.repo_state(&t.id).and_then(|s| s.last_attempt);
        let latest = overview.tracked.iter().filter_map(last_attempt).max();
        let due = latest.unwrap_or(started) + gap;
        // Never-attempted repos sort first (`None < Some`); `min_by_key`
        // keeps the first of equals, so ties go by config order.
        let repo = overview.tracked.iter().min_by_key(|t| last_attempt(t))?;
        Some((due, repo.clone()))
    }

    /// Resolves once the repo isn't tracked any more.
    async fn until_untracked(&self, id: &RepoId) {
        loop {
            // Registered before checking, so an untrack right after the
            // check still wakes it.
            let notified = self.untracked.notified();
            let mut notified = std::pin::pin!(notified);
            notified.as_mut().enable();
            if !self.overview.lock().unwrap().is_tracked(id) {
                return;
            }
            notified.await;
        }
    }

    /// Fetches the repo and records the result, unless the repo is untracked
    /// before the fetch finishes.
    async fn fetch(&self, repo: Tracked) {
        // Held until the state is saved, so deleting an archive never
        // interleaves with fetching it.
        let _exclusive = self.archives.exclusive().await;
        let attempt = whole_seconds(self.clock.now());
        let timer = Instant::now();
        let token = self.tokens.for_repo(&repo.id);
        let result = tokio::select! {
            biased;
            () = self.until_untracked(&repo.id) => {
                tracing::info!(repo = %repo.id, "not fetching: no longer tracked");
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

        let state = {
            let mut overview = self.overview.lock().unwrap();
            overview.last_fetch_duration = Some(duration);
            overview.fetch_first.retain(|id| *id != repo.id);
            let (result, preserved) = match result {
                Ok(preserved) => (Ok(()), preserved),
                Err(e) => (Err(e), Vec::new()),
            };
            if !preserved.is_empty() {
                overview.rewrites += 1;
                tracing::warn!(
                    repo = %repo.id,
                    preserved = ?preserved,
                    "upstream rewrote history; preserved the old tips",
                );
            }
            if let Err(e) = &result {
                overview.fetch_errors.count(e.status());
            }
            let entry = overview.state.repos.entry(repo.id.to_string()).or_default();
            entry.record_attempt(attempt, size);
            let change = entry.record_result(attempt, &result);
            match (&result, change) {
                (Ok(()), Some(change)) => tracing::info!(
                    repo = %repo.id,
                    from = %change.from.map_or("none", Status::as_str),
                    to = %change.to.as_str(),
                    "status changed",
                ),
                (Err(e), Some(change)) => tracing::warn!(
                    repo = %repo.id,
                    from = %change.from.map_or("none", Status::as_str),
                    to = %change.to.as_str(),
                    reason = e.reason(),
                    "status changed",
                ),
                (Err(e), None) => tracing::warn!(
                    repo = %repo.id,
                    status = %e.status().as_str(),
                    reason = e.reason(),
                    "status unchanged",
                ),
                (Ok(()), None) => {}
            }
            if result.is_ok() {
                tracing::info!(repo = %repo.id, duration_ms = duration.as_millis(), "fetched");
            }
            overview.state.clone()
        };
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
