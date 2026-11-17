//! What the scheduler knows, shared with the web UI and `/metrics`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime};

use tokio::sync::watch;

use crate::archive::FetchError;
use crate::identity::RepoId;
use crate::state::{RepoState, State, Status};

pub type SharedOverview = Arc<Overview>;

/// What the app remembers about its archives, shared between the scheduler,
/// the web UI and `/metrics`. It owns its lock, saves the state itself
/// whenever it changes, and tells whoever watches when something the pages
/// show changed.
#[derive(Debug)]
pub struct Overview {
    inner: Mutex<Inner>,
    save_lock: Mutex<()>,
    state_path: PathBuf,
    /// Sent to after each change the pages show.
    changes: watch::Sender<()>,
}

/// Notices of changes to what the pages show, from [`Overview::changes`].
pub struct Changes(watch::Receiver<()>);

impl Changes {
    /// Resolves at the next change, or at once if one came since the last
    /// call. Changes in a burst may resolve it only once.
    pub async fn changed(&mut self) {
        // The sender lives in the Overview, which outlives every watcher that
        // holds it; if it's gone, nothing will change any more.
        if self.0.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[derive(Debug, Default)]
struct Inner {
    /// In config order, one entry per repo identity.
    tracked: Vec<Tracked>,
    state: State,
    next_fetch: Option<SystemTime>,
    last_fetch_duration: Option<Duration>,
    fetch_errors: FetchErrors,
    /// Repos to fetch before any scheduled fetch, in order: repos added in
    /// the UI that wait for their clone.
    fetch_first: Vec<RepoId>,
    rewrites: u64,
    /// Why the config file can't be used right now, for the warning banner.
    /// The last good config stays in use meanwhile.
    config_error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FetchErrors {
    pub unavailable: u64,
    pub fetch_failed: u64,
}

impl FetchErrors {
    /// Counts a failed fetch by the status it leads to.
    pub fn count(&mut self, status: Status) {
        match status {
            Status::Available => {}
            Status::Unavailable => self.unavailable += 1,
            Status::FetchFailed => self.fetch_failed += 1,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tracked {
    /// As written in the config.
    pub url: String,
    pub id: RepoId,
}

/// One repo the app knows: tracked, or an archive kept after untracking.
#[derive(Debug, Clone)]
pub struct RepoView {
    /// The URL as written in the config for a tracked repo, else the
    /// identity.
    pub title: String,
    pub id: RepoId,
    pub tracked: bool,
    /// As last known, if the app ever fetched it.
    pub state: Option<RepoState>,
}

/// Every repo the app knows, with what it remembers about fetching.
#[derive(Debug, Clone)]
pub struct View {
    /// In config order.
    pub tracked: Vec<RepoView>,
    /// Untracked archives, by `<host>/<path>`.
    pub untracked: Vec<RepoView>,
    pub next_fetch: Option<SystemTime>,
    pub last_fetch_duration: Option<Duration>,
    /// Failed fetches since the app started.
    pub fetch_errors: FetchErrors,
    /// Fetches that found a rewrite since the app started.
    pub rewrites: u64,
    pub config_error: Option<String>,
}

impl View {
    /// Every repo: the tracked ones, then the untracked archives.
    pub fn repos(&self) -> impl Iterator<Item = &RepoView> {
        self.tracked.iter().chain(&self.untracked)
    }
}

impl Overview {
    pub fn new(tracked: Vec<Tracked>, state: State, state_path: PathBuf) -> SharedOverview {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                tracked,
                state,
                ..Inner::default()
            }),
            save_lock: Mutex::new(()),
            state_path,
            changes: watch::Sender::new(()),
        })
    }

    /// Watches for changes to what the pages show: a fetch result, the
    /// tracked list, the config error, or an untracked archive deleted.
    /// Only changes after this call count.
    pub fn changes(&self) -> Changes {
        Changes(self.changes.subscribe())
    }

    fn announce(&self) {
        self.changes.send_replace(());
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    /// Every repo the app knows: the tracked ones and the archives `on_disk`
    /// (by `<host>/<path>`) that no tracked repo uses.
    pub fn view(&self, on_disk: &[String]) -> View {
        let inner = self.inner();
        let tracked = inner
            .tracked
            .iter()
            .map(|t| RepoView {
                title: t.url.clone(),
                id: t.id.clone(),
                tracked: true,
                state: inner.repo_state(&t.id).cloned(),
            })
            .collect();
        let untracked = inner
            .untracked(on_disk)
            .into_iter()
            .map(|(id, state)| RepoView {
                title: id.to_string(),
                id,
                tracked: false,
                state,
            })
            .collect();
        View {
            tracked,
            untracked,
            next_fetch: inner.next_fetch,
            last_fetch_duration: inner.last_fetch_duration,
            fetch_errors: inner.fetch_errors.clone(),
            rewrites: inner.rewrites,
            config_error: inner.config_error.clone(),
        }
    }

    pub fn is_tracked(&self, id: &RepoId) -> bool {
        self.inner().tracked.iter().any(|t| t.id == *id)
    }

    /// Takes the config's repo list after a repo was untracked or found
    /// missing from it. Repos no longer tracked leave the queue.
    pub fn set_tracked(&self, repos: &[String]) {
        let changed = self.inner().set_tracked(repos);
        if changed {
            self.announce();
        }
    }

    /// Takes the config's repo list after `id` was added, and queues it to
    /// be fetched before any scheduled fetch, due `now`.
    pub fn track_first(&self, repos: &[String], id: &RepoId, now: SystemTime) {
        {
            let mut inner = self.inner();
            inner.set_tracked(repos);
            inner.fetch_first.push(id.clone());
            inner.next_fetch = Some(now);
        }
        self.announce();
    }

    /// Records the config error, or that there is none. Returns whether that
    /// changed what was recorded.
    pub fn set_config_error(&self, error: Option<String>) -> bool {
        let changed = {
            let mut inner = self.inner();
            let changed = inner.config_error != error;
            inner.config_error = error;
            changed
        };
        if changed {
            self.announce();
        }
        changed
    }

    /// When the next fetch is due and which repo it fetches, also recorded
    /// for `/metrics`. A repo that jumped the queue is due `now`. Reading
    /// the queue and recording the result share one lock, so a repo jumping
    /// the queue meanwhile isn't hidden by a stale due time.
    pub fn next_fetch(
        &self,
        interval: Duration,
        started: SystemTime,
        now: SystemTime,
    ) -> Option<(SystemTime, Tracked)> {
        let mut inner = self.inner();
        let next = inner.next_fetch(interval, started, now);
        inner.next_fetch = next.as_ref().map(|(due, _)| *due);
        next
    }

    /// Records a finished fetch of `id` begun at `attempt`, logs what it
    /// means, takes the repo off the fetch-first queue and saves the state.
    /// `result` holds the preserved refs, if the fetch found a rewrite.
    pub fn record_fetch(
        &self,
        id: &RepoId,
        attempt: SystemTime,
        duration: Duration,
        size: Option<u64>,
        result: Result<Vec<String>, FetchError>,
    ) {
        self.change(|inner| {
            inner.record_fetch(id, attempt, duration, size, result);
            inner.fetch_first.retain(|queued| queued != id);
        });
    }

    /// Forgets the state of every identity that shares the archive of `id`,
    /// and saves. Called once the archive is deleted, so that counts as a
    /// change even with no state to forget.
    pub fn forget(&self, id: &RepoId) {
        self.change(|inner| {
            inner
                .state
                .repos
                .retain(|key, _| RepoId::from_url(key).map_or(true, |other| other != *id));
        });
    }

    /// Applies `change`, saves the resulting state and announces the change.
    /// The save lock is held from taking a copy of the state until it's
    /// written, so an older copy never overwrites a newer one.
    fn change(&self, change: impl FnOnce(&mut Inner)) {
        let _save = self.save_lock.lock().unwrap_or_else(|e| e.into_inner());
        let state = {
            let mut inner = self.inner();
            change(&mut inner);
            inner.state.clone()
        };
        if let Err(e) = state.save(&self.state_path) {
            tracing::error!(error = %format!("{e:#}"), "couldn't save state");
        }
        self.announce();
    }
}

impl Inner {
    fn repo_state(&self, id: &RepoId) -> Option<&RepoState> {
        self.state.repos.get(id.as_str())
    }

    /// Returns whether the tracked list changed.
    fn set_tracked(&mut self, repos: &[String]) -> bool {
        let tracked = tracked(repos);
        let changed = self.tracked != tracked;
        self.tracked = tracked;
        let Self {
            tracked,
            fetch_first,
            ..
        } = self;
        fetch_first.retain(|id| tracked.iter().any(|t| t.id == *id));
        changed
    }

    /// The repo to fetch before any scheduled fetch: the first queued repo
    /// that is still tracked.
    fn fetch_first(&self) -> Option<&Tracked> {
        self.fetch_first
            .iter()
            .find_map(|id| self.tracked.iter().find(|t| &t.id == id))
    }

    fn next_fetch(
        &self,
        interval: Duration,
        started: SystemTime,
        now: SystemTime,
    ) -> Option<(SystemTime, Tracked)> {
        if let Some(repo) = self.fetch_first() {
            return Some((now, repo.clone()));
        }
        let count = u32::try_from(self.tracked.len()).unwrap_or(u32::MAX);
        if count == 0 {
            return None;
        }
        let gap = interval / count;
        let last_attempt = |t: &Tracked| self.repo_state(&t.id).and_then(|s| s.last_attempt);
        let latest = self.tracked.iter().filter_map(last_attempt).max();
        let due = latest.unwrap_or(started) + gap;
        // Never-attempted repos sort first (`None < Some`); `min_by_key`
        // keeps the first of equals, so ties go by config order.
        let repo = self.tracked.iter().min_by_key(|t| last_attempt(t))?;
        Some((due, repo.clone()))
    }

    fn record_fetch(
        &mut self,
        id: &RepoId,
        attempt: SystemTime,
        duration: Duration,
        size: Option<u64>,
        result: Result<Vec<String>, FetchError>,
    ) {
        self.last_fetch_duration = Some(duration);
        let (result, preserved) = match result {
            Ok(preserved) => (Ok(()), preserved),
            Err(e) => (Err(e), Vec::new()),
        };
        if !preserved.is_empty() {
            self.rewrites += 1;
            tracing::warn!(
                repo = %id,
                preserved = ?preserved,
                "upstream rewrote history; preserved the old tips",
            );
        }
        if let Err(e) = &result {
            self.fetch_errors.count(e.status());
        }
        let entry = self.state.repos.entry(id.to_string()).or_default();
        entry.record_attempt(attempt, size);
        let change = entry.record_result(attempt, &result);
        match (&result, change) {
            (Ok(()), Some(change)) => tracing::info!(
                repo = %id,
                from = %change.from.map_or("none", Status::as_str),
                to = %change.to.as_str(),
                "status changed",
            ),
            (Err(e), Some(change)) => tracing::warn!(
                repo = %id,
                from = %change.from.map_or("none", Status::as_str),
                to = %change.to.as_str(),
                reason = e.reason(),
                "status changed",
            ),
            (Err(e), None) => tracing::warn!(
                repo = %id,
                status = %e.status().as_str(),
                reason = e.reason(),
                "status unchanged",
            ),
            (Ok(()), None) => {}
        }
        if result.is_ok() {
            tracing::info!(repo = %id, duration_ms = duration.as_millis(), "fetched");
        }
    }

    /// The archives on disk, by `<host>/<path>`, that no tracked repo uses,
    /// each named by the identity of its state entry, with that state. An
    /// archive with no state entry is named as if fetched over HTTPS.
    fn untracked(&self, on_disk: &[String]) -> Vec<(RepoId, Option<RepoState>)> {
        on_disk
            .iter()
            .filter(|archive| {
                !self
                    .tracked
                    .iter()
                    .any(|t| t.id.host_and_path() == archive.as_str())
            })
            .filter_map(|archive| {
                let known = self
                    .state
                    .repos
                    .iter()
                    .filter_map(|(key, state)| {
                        let id = RepoId::from_url(key).ok()?;
                        (id.host_and_path() == archive.as_str()).then_some((id, state))
                    })
                    // Entries for other schemes share the archive; the
                    // latest attempt is the one that last touched it.
                    .max_by_key(|(_, state)| state.last_attempt);
                match known {
                    Some((id, state)) => Some((id, Some(state.clone()))),
                    None => Some((RepoId::from_url(&format!("https://{archive}")).ok()?, None)),
                }
            })
            .collect()
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    /// An Overview tracking `urls`, saving its state in a fresh directory.
    fn overview(urls: &[String]) -> (tempfile::TempDir, SharedOverview) {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("state.json");
        (dir, Overview::new(tracked(urls), State::default(), path))
    }

    /// Whether `changes` has a change waiting, without waiting for one.
    async fn announced(changes: &mut Changes) -> bool {
        tokio::time::timeout(Duration::ZERO, changes.changed())
            .await
            .is_ok()
    }

    #[tokio::test]
    async fn deleting_an_untracked_archive_is_announced() {
        let (_dir, overview) = overview(&[]);
        let mut changes = overview.changes();
        overview.forget(&RepoId::from_url("https://example.com/a/one").unwrap());
        assert!(announced(&mut changes).await);
    }

    #[tokio::test]
    async fn only_a_changed_tracked_list_is_announced() {
        let one = "https://example.com/a/one".to_owned();
        let (_dir, overview) = overview(std::slice::from_ref(&one));
        let mut changes = overview.changes();

        overview.set_tracked(std::slice::from_ref(&one));
        assert!(!announced(&mut changes).await, "the same list");

        // Another spelling of the same repo shows another title.
        overview.set_tracked(&["http://example.com/a/one".to_owned()]);
        assert!(announced(&mut changes).await, "the URL as written changed");
    }
}
