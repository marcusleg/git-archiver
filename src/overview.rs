//! What the scheduler knows, shared with the web UI and `/metrics`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::identity::RepoId;
use crate::state::{RepoState, State};

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
}

#[derive(Debug, Default)]
pub struct FetchErrors {
    pub unavailable: u64,
    pub fetch_failed: u64,
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
