//! What the scheduler knows, shared with the web UI and `/metrics`.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::identity::RepoId;
use crate::state::{RepoState, State, Status};

pub type SharedOverview = Arc<Mutex<Overview>>;

#[derive(Debug, Default)]
pub struct Overview {
    /// In config order, one entry per repo identity.
    pub tracked: Vec<Tracked>,
    pub state: State,
    pub next_fetch: Option<SystemTime>,
    pub last_fetch_duration: Option<Duration>,
    /// Failed fetches since the app started.
    pub fetch_errors: FetchErrors,
    /// Repos to fetch before any scheduled fetch, in order: repos added in
    /// the UI that wait for their clone.
    pub fetch_first: Vec<RepoId>,
    /// Fetches that found a rewrite since the app started.
    pub rewrites: u64,
    /// Why the config file can't be used right now, for the warning banner.
    /// The last good config stays in use meanwhile.
    pub config_error: Option<String>,
}

#[derive(Debug, Default)]
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

/// An archive on disk whose repo is no longer tracked.
#[derive(Debug, Clone)]
pub struct Untracked {
    pub id: RepoId,
    /// As last known, if the app ever fetched it.
    pub state: Option<RepoState>,
}

#[derive(Debug, Clone)]
pub struct Tracked {
    /// As written in the config.
    pub url: String,
    pub id: RepoId,
}

impl Overview {
    pub fn repo_state(&self, id: &RepoId) -> Option<&RepoState> {
        self.state.repos.get(id.as_str())
    }

    /// Takes the config's repo list after a repo was tracked, untracked or
    /// found missing from it. Repos no longer tracked leave the queue.
    pub fn set_tracked(&mut self, repos: &[String]) {
        self.tracked = tracked(repos);
        let Self {
            tracked,
            fetch_first,
            ..
        } = self;
        fetch_first.retain(|id| tracked.iter().any(|t| t.id == *id));
    }

    pub fn is_tracked(&self, id: &RepoId) -> bool {
        self.tracked.iter().any(|t| t.id == *id)
    }

    /// The archives on disk, by `<host>/<path>`, that no tracked repo uses,
    /// each named by the identity of its state entry. An archive with no
    /// state entry is named as if fetched over HTTPS.
    pub fn untracked(&self, on_disk: &[String]) -> Vec<Untracked> {
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
                    Some((id, state)) => Some(Untracked {
                        id,
                        state: Some(state.clone()),
                    }),
                    None => Some(Untracked {
                        id: RepoId::from_url(&format!("https://{archive}")).ok()?,
                        state: None,
                    }),
                }
            })
            .collect()
    }

    /// Forgets the state of every identity that shares the archive of `id`.
    pub fn forget(&mut self, id: &RepoId) {
        self.state
            .repos
            .retain(|key, _| RepoId::from_url(key).map_or(true, |other| other != *id));
    }

    /// Each tracked repo with its state, if it has any yet.
    pub fn tracked_with_state(&self) -> impl Iterator<Item = (&Tracked, Option<&RepoState>)> {
        self.tracked.iter().map(|t| (t, self.repo_state(&t.id)))
    }
}

/// Held from taking a copy of the state until it's saved, so an older copy
/// never overwrites a newer one.
static SAVE_LOCK: Mutex<()> = Mutex::new(());

/// Saves the current state to `path`, logging a failure.
pub fn save_state(overview: &SharedOverview, path: &Path) {
    let _lock = SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let state = overview.lock().unwrap().state.clone();
    if let Err(e) = state.save(path) {
        tracing::error!(error = %format!("{e:#}"), "couldn't save state");
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
