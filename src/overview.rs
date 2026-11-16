//! What the scheduler knows, shared with the web UI and `/metrics`.

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

    /// Takes the config's new repo list after the repo `id` was added, and
    /// queues that repo to fetch as soon as no other fetch is running. The
    /// scheduler must be woken to notice.
    pub fn add_tracked(&mut self, repos: &[String], id: RepoId, now: SystemTime) {
        self.tracked = tracked(repos);
        self.fetch_first.push(id);
        self.next_fetch = Some(now);
    }

    /// Each tracked repo with its state, if it has any yet.
    pub fn tracked_with_state(&self) -> impl Iterator<Item = (&Tracked, Option<&RepoState>)> {
        self.tracked.iter().map(|t| (t, self.repo_state(&t.id)))
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
