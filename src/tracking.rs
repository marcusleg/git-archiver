//! Which repos are tracked: the tracked set, the queue of newly tracked repos
//! to fetch first, the signal that wakes the scheduler, and the next-fetch
//! time a newly tracked repo gets. Tracking and reloading the config go
//! through here; the config lock and its edit callbacks stay inside.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;

use crate::archive;
use crate::clock::Clock;
use crate::config::{self, EditError, RepoListChange, Tokens};
use crate::identity::RepoId;
use crate::overview::{Overview, SharedOverview};

/// How long tracking waits for upstream to answer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Tracking {
    config_path: PathBuf,
    clock: Arc<dyn Clock>,
    overview: SharedOverview,
    /// Wakes the scheduler from waiting, after the tracked set changed or a
    /// repo jumped the queue. Stores a permit, so a wake while the scheduler
    /// is busy isn't lost.
    wake: Notify,
    /// Notified after a repo was untracked, to stop its fetch.
    untracked: Notify,
}

/// How asking to track a URL ended.
#[derive(Debug, PartialEq, Eq)]
pub enum TrackOutcome {
    /// The repo is tracked now and queued for its first fetch.
    Tracked,
    /// A repo with the same archive is tracked, under this URL.
    AlreadyTracked(String),
    /// The config wouldn't accept the URL; why, as the rest of a sentence
    /// that starts with the URL.
    Invalid(&'static str),
    /// Upstream didn't answer as a git repo; why.
    CouldNotBeChecked(String),
    /// The config file holds no valid config, so it can't be changed.
    ConfigInvalid,
    /// The config file can't be read or written; why.
    ConfigUnwritable(String),
}

/// How asking to untrack a repo ended.
#[derive(Debug, PartialEq, Eq)]
pub enum UntrackOutcome {
    /// The repo was tracked and isn't now.
    Untracked,
    /// The repo wasn't tracked, e.g. a form sent twice.
    NotTracked,
    ConfigInvalid,
    ConfigUnwritable(String),
}

impl Tracking {
    pub fn new(config_path: PathBuf, clock: Arc<dyn Clock>, overview: SharedOverview) -> Self {
        Self {
            config_path,
            clock,
            overview,
            wake: Notify::new(),
            untracked: Notify::new(),
        }
    }

    /// Checks `url`, makes sure upstream answers, writes it into the config
    /// and queues its clone to run before any scheduled fetch.
    pub async fn track(&self, url: &str) -> TrackOutcome {
        let id = match config::check_repo_url(url) {
            Ok(id) => id,
            Err(problem) => return TrackOutcome::Invalid(problem),
        };
        let token = match config::check_add(&self.config_path, &id) {
            Ok(token) => token,
            Err(e) => return self.track_error(e),
        };

        let deadline = self.clock.now() + PROBE_TIMEOUT;
        let probe = tokio::select! {
            result = archive::probe(url, token.as_ref()) => result,
            () = self.clock.sleep_until(deadline) => {
                Err("it didn’t answer within a minute.".to_owned())
            }
        };
        if let Err(message) = probe {
            return TrackOutcome::CouldNotBeChecked(message);
        }

        let edited = config::edit_repo_list(
            &self.config_path,
            RepoListChange::Add { url, id: &id },
            |repos| {
                let mut overview = self.overview.lock().unwrap();
                overview.set_tracked(repos);
                overview.fetch_first.push(id.clone());
                overview.next_fetch = Some(self.clock.now());
            },
        );
        if let Err(e) = edited {
            return self.track_error(e);
        }
        tracing::info!(repo = %id, "added");
        self.wake.notify_one();
        TrackOutcome::Tracked
    }

    fn track_error(&self, error: EditError) -> TrackOutcome {
        match error {
            EditError::AlreadyTracked(existing) => TrackOutcome::AlreadyTracked(existing),
            EditError::Invalid(e) => {
                log_edit_error("add the repo", &e);
                TrackOutcome::ConfigInvalid
            }
            EditError::Io(e) => {
                log_edit_error("add the repo", &e);
                TrackOutcome::ConfigUnwritable(e.root_cause().to_string())
            }
        }
    }

    /// Removes the repo from the config and stops any fetch of it.
    pub fn untrack(&self, id: &RepoId) -> UntrackOutcome {
        // A repo removed from the file by hand still shows as tracked until
        // the scheduler re-reads it; the person saw it tracked, so it counts.
        let shown_tracked = self.is_tracked(id);
        let edited =
            config::edit_repo_list(&self.config_path, RepoListChange::Remove { id }, |repos| {
                self.overview.lock().unwrap().set_tracked(repos)
            });
        let removed = match edited {
            Ok(edited) => edited == config::Edited::Changed,
            Err(EditError::Invalid(e)) => {
                log_edit_error("untrack the repo", &e);
                return UntrackOutcome::ConfigInvalid;
            }
            Err(EditError::Io(e)) => {
                log_edit_error("untrack the repo", &e);
                return UntrackOutcome::ConfigUnwritable(e.root_cause().to_string());
            }
            // Only adding refuses a tracked repo; if that ever changes, the
            // config is the problem, not the person.
            Err(EditError::AlreadyTracked(_)) => return UntrackOutcome::ConfigInvalid,
        };
        if !removed && !shown_tracked {
            return UntrackOutcome::NotTracked;
        }
        tracing::info!(repo = %id, "untracked");
        self.untracked.notify_waiters();
        self.wake.notify_one();
        UntrackOutcome::Untracked
    }

    /// Re-reads the config's repo list and tokens. Queued repos no longer
    /// tracked leave the queue. An unreadable or invalid config changes
    /// nothing; the error is for the caller to report.
    pub fn reload(&self) -> Result<Tokens, String> {
        config::load_locked(&self.config_path, |config| {
            let config = config.map_err(|e| format!("{e:#}"))?;
            self.overview.lock().unwrap().set_tracked(&config.repos);
            Ok(config.tokens)
        })
    }

    pub fn is_tracked(&self, id: &RepoId) -> bool {
        self.overview.lock().unwrap().is_tracked(id)
    }

    /// The repo to fetch before any scheduled fetch: the first queued repo
    /// that is still tracked.
    pub fn fetch_first(overview: &Overview) -> Option<&crate::overview::Tracked> {
        overview
            .fetch_first
            .iter()
            .find_map(|id| overview.tracked.iter().find(|t| &t.id == id))
    }

    /// Takes the repo off the fetch-first queue, once it has been fetched.
    pub fn fetched(&self, id: &RepoId) {
        self.overview
            .lock()
            .unwrap()
            .fetch_first
            .retain(|queued| queued != id);
    }

    /// Resolves when something changed that the scheduler should look at: a
    /// repo was tracked or untracked.
    pub async fn changed(&self) {
        self.wake.notified().await;
    }

    /// Resolves once the repo isn't tracked any more.
    pub async fn until_untracked(&self, id: &RepoId) {
        loop {
            // Registered before checking, so an untrack right after the
            // check still wakes it.
            let notified = self.untracked.notified();
            let mut notified = std::pin::pin!(notified);
            notified.as_mut().enable();
            if !self.is_tracked(id) {
                return;
            }
            notified.await;
        }
    }
}

fn log_edit_error(action: &str, error: &anyhow::Error) {
    tracing::error!(error = %format!("{error:#}"), "couldn't {action} in the config");
}
