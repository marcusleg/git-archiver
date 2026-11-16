//! Archives: one bare git repo per repo identity at `repos/<host>/<path>.git`,
//! created and updated only through the `git` CLI (ADR-0001).
//!
//! A fetch works in two steps. It first brings upstream's default branch and
//! tags into a staging namespace inside the archive. Then it keeps every
//! archived tip a rewrite would lose as a preserved ref (ADR-0002), and only
//! after that moves the archived refs to the staged state and clears staging.
//! A fetch interrupted at any point has lost nothing: the next one clears the
//! leftover staging refs, finds the same rewrite and finishes it, preserving
//! the same tips again under its own time if the first got that far.
//!
//! A failed fetch is classified from git's exit and stderr alone (ADR-0001):
//! upstream refusing the repo makes it Unavailable, anything else is a
//! transient Fetch failed.

use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::{Context, bail};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::Token;
use crate::identity::RepoId;
use crate::state::Status;

/// Where a fetch stages upstream's refs. Never meant for clients.
pub(crate) const STAGING: &str = "refs/staging/";
const PRESERVED: &str = "refs/preserved/";

pub struct Archives {
    root: PathBuf,
    /// One lock per archive, held while fetching or deleting it.
    busy: std::sync::Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

/// Facts read from an archive.
#[derive(Debug, Clone, Default)]
pub struct Inspection {
    /// Only set once the archive holds that branch.
    pub default_branch: Option<String>,
    pub tip: Option<String>,
    /// By name, so oldest first, with the object each holds. `None` until
    /// the archive exists.
    pub preserved: Option<BTreeMap<String, String>>,
}

/// Why a fetch failed. Neither kind touches the archive's refs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// Upstream refused the repo with a 4xx response.
    Unavailable {
        /// The HTTP status, as git reported it or as its message implies.
        http_status: u16,
        reason: String,
    },
    /// A transient failure that says nothing about the repo's fate.
    Failed { reason: String },
}

impl FetchError {
    /// The status this failure gives the repo.
    pub fn status(&self) -> Status {
        match self {
            Self::Unavailable { .. } => Status::Unavailable,
            Self::Failed { .. } => Status::FetchFailed,
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::Unavailable { reason, .. } | Self::Failed { reason } => reason,
        }
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for FetchError {}

/// Sorts a failed git command talking to upstream by its stderr. The reason
/// is git's message: the `remote:` lines plus the final error.
fn classify(stderr: &str) -> FetchError {
    let lines: Vec<&str> = stderr.lines().map(str::trim_end).collect();
    let last_error = lines
        .iter()
        .rev()
        .find(|line| line.starts_with("fatal: ") || line.starts_with("error: "));
    let reason = lines
        .iter()
        .filter(|line| line.starts_with("remote:"))
        .chain(last_error)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let reason = if reason.is_empty() {
        stderr.trim().to_owned()
    } else {
        reason
    };
    let error = last_error.copied().unwrap_or_default();
    let http_status =
        if let Some((_, code)) = error.rsplit_once("The requested URL returned error: ") {
            code.get(..3).and_then(|code| code.parse().ok())
        } else if error.starts_with("fatal: repository '") && error.ends_with("' not found") {
            // Git prints this instead of the 404 status.
            Some(404)
        } else if error.starts_with("fatal: could not read Username for ")
            || error.starts_with("fatal: Authentication failed for ")
        {
            // Git asked for credentials and got none, so the server answered 401.
            Some(401)
        } else {
            None
        };
    match http_status {
        // 408 and 429 ask the client to come back later, which says nothing
        // about the repo's fate.
        Some(http_status @ 400..500) if http_status != 408 && http_status != 429 => {
            FetchError::Unavailable {
                http_status,
                reason,
            }
        }
        _ => FetchError::Failed { reason },
    }
}

impl Archives {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("repos"),
            busy: std::sync::Mutex::default(),
        }
    }

    /// Waits until no fetch or delete of the repo's archive runs, and keeps
    /// others waiting until the guard is dropped. Hold it from a git
    /// operation until its result is saved.
    pub async fn exclusive(&self, id: &RepoId) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = self
            .busy
            .lock()
            .unwrap()
            .entry(id.archive_path())
            .or_default()
            .clone();
        lock.lock_owned().await
    }

    /// The `repos/` directory every archive lives under.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path(&self, id: &RepoId) -> PathBuf {
        self.root.join(id.archive_path())
    }

    /// Clones the repo into its archive, or updates the archive from
    /// upstream. Before an archived ref moves or goes in a way that would
    /// lose history (a rewrite), its old tip is kept as a preserved ref under
    /// `refs/preserved/<now>/`. Returns the preserved refs it created. A repo
    /// upstream refuses on its first fetch gets no archive. The token, if any, is
    /// sent with every request to upstream.
    pub async fn fetch(
        &self,
        id: &RepoId,
        url: &str,
        now: SystemTime,
        token: Option<&Token>,
    ) -> Result<Vec<String>, FetchError> {
        self.try_fetch(id, url, now, token).await.map_err(|e| {
            e.downcast::<FetchError>()
                .unwrap_or_else(|e| FetchError::Failed {
                    reason: format!("{e:#}"),
                })
        })
    }

    async fn try_fetch(
        &self,
        id: &RepoId,
        url: &str,
        now: SystemTime,
        token: Option<&Token>,
    ) -> anyhow::Result<Vec<String>> {
        let archive = self.path(id);
        let default_branch = upstream_default_branch(url, token).await?;
        create_if_missing(&archive).await?;

        clear_staging(&archive).await?;
        let mut refspecs = vec![format!("+refs/tags/*:{STAGING}tags/*")];
        if let Some(branch) = &default_branch {
            refspecs.push(format!("+refs/heads/{branch}:{STAGING}heads/{branch}"));
        }
        let mut args = vec![
            "fetch",
            "--quiet",
            "--no-tags",
            "--no-write-fetch-head",
            "--no-auto-gc",
            url,
        ];
        args.extend(refspecs.iter().map(String::as_str));
        git_upstream(&archive, &args, url, token).await?;

        let staged = list_refs(&archive, &[STAGING]).await?;
        let archived = list_refs(&archive, &["refs/heads/", "refs/tags/"]).await?;
        let upstream: BTreeMap<String, &str> = staged
            .iter()
            .map(|(name, sha)| (format!("refs/{}", &name[STAGING.len()..]), sha.as_str()))
            .collect();
        let stamp = ref_timestamp(now);

        // Each step below is its own ref transaction, because the files ref
        // backend commits a transaction one ref at a time. In this order, a
        // fetch that dies between or during any of them has preserved every
        // tip it was about to lose, and the next fetch finishes the job.
        let mut preserve = String::new();
        let mut delete = String::new();
        let mut update = String::new();
        let mut preserved = Vec::new();
        for (name, old) in &archived {
            let new = upstream.get(name).copied();
            let rewritten = match new {
                Some(new) if new == old.as_str() => continue,
                Some(new) if name.starts_with("refs/heads/") => {
                    !is_ancestor(&archive, old, new).await?
                }
                // Gone upstream, or a branch that is no longer the default.
                _ => true,
            };
            if rewritten {
                let preserved_ref = format!("{PRESERVED}{stamp}/{}", &name["refs/".len()..]);
                preserve.push_str(&format!("create {preserved_ref} {old}\n"));
                preserved.push(preserved_ref);
            }
            match new {
                Some(new) => update.push_str(&format!("update {name} {new} {old}\n")),
                None => delete.push_str(&format!("delete {name} {old}\n")),
            }
        }
        for (name, new) in &upstream {
            if !archived.contains_key(name) {
                update.push_str(&format!("create {name} {new}\n"));
            }
        }
        for (name, sha) in &staged {
            update.push_str(&format!("delete {name} {sha}\n"));
        }

        // 1. Keep what the rewrite would lose.
        git_with_input(&archive, &["update-ref", "--stdin"], &preserve).await?;
        // 2. Follow upstream's default branch, even before it exists here.
        if let Some(branch) = &default_branch {
            git(
                &archive,
                &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
            )
            .await?;
        }
        // 3. Remove refs gone upstream first, so a new ref may take a name
        //    nested under a deleted one (tag `v1` replaced by `v1/rc`).
        git_with_input(&archive, &["update-ref", "--stdin"], &delete).await?;
        // 4. Take upstream's state and clear staging.
        git_with_input(&archive, &["update-ref", "--stdin"], &update).await?;
        Ok(preserved)
    }

    /// Reads the default branch, its tip and the preserved refs from the
    /// archive, if there is one.
    pub async fn inspect(&self, id: &RepoId) -> anyhow::Result<Inspection> {
        let archive = self.path(id);
        if !archive.exists() {
            return Ok(Inspection::default());
        }
        let preserved = self.preserved(id).await?;
        let head = git(&archive, &["symbolic-ref", "HEAD"]).await?;
        let head = head.trim();
        let Some(tip) = rev_parse(&archive, head).await? else {
            return Ok(Inspection {
                preserved,
                ..Inspection::default()
            });
        };
        Ok(Inspection {
            default_branch: head.strip_prefix("refs/heads/").map(str::to_owned),
            tip: Some(tip),
            preserved,
        })
    }

    /// Whether the repo has an archive on disk yet.
    pub fn exists(&self, id: &RepoId) -> bool {
        self.path(id).exists()
    }

    /// The archive's preserved refs by name (so oldest first) with the object
    /// each holds, or `None` if there is no archive yet.
    pub async fn preserved(&self, id: &RepoId) -> anyhow::Result<Option<BTreeMap<String, String>>> {
        let archive = self.path(id);
        if !archive.exists() {
            return Ok(None);
        }
        Ok(Some(list_refs(&archive, &[PRESERVED]).await?))
    }

    /// Every archive on disk, by `<host>/<path>`, sorted.
    pub async fn list(&self) -> Vec<String> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            let mut found = Vec::new();
            if let Err(e) = find_archives(&root, "", &mut found)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(error = %e, "couldn't list all archives");
            }
            found.sort();
            found
        })
        .await
        .unwrap_or_default()
    }

    /// Deletes the archive from disk, if there is one. Moves it aside first,
    /// so a crash midway never leaves a partial archive behind, then removes
    /// the directories left empty above it.
    pub async fn delete(&self, id: &RepoId) -> anyhow::Result<()> {
        let archive = self.path(id);
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            if !archive.exists() {
                return Ok(());
            }
            let parent = archive.parent().expect("archive path has a parent");
            let doomed = tempfile::Builder::new()
                .prefix(".delete-")
                .tempdir_in(parent)
                .with_context(|| format!("couldn't create a directory in {}", parent.display()))?;
            let target = doomed.path().join("archive");
            std::fs::rename(&archive, &target)
                .with_context(|| format!("couldn't delete archive {}", archive.display()))?;
            // Moved out of `repos/`'s archives, it's gone for the app.
            let leftover = doomed.path().to_owned();
            if let Err(e) = doomed.close() {
                tracing::warn!(
                    path = %leftover.display(),
                    error = %e,
                    "couldn't remove a deleted archive's files; delete them by hand",
                );
            }
            let mut dir = parent;
            while dir != root && std::fs::remove_dir(dir).is_ok() {
                dir = dir.parent().expect("inside repos/");
            }
            Ok(())
        })
        .await
        .context("couldn't delete the archive")?
    }

    /// The archive's size on disk in bytes, if it exists.
    pub async fn size(&self, id: &RepoId) -> Option<u64> {
        let archive = self.path(id);
        tokio::task::spawn_blocking(move || dir_size(&archive).ok())
            .await
            .ok()
            .flatten()
    }
}

/// Asks upstream for its `HEAD` without fetching anything, to check that the
/// URL answers like a git repo. The token, if any, is sent the way a fetch
/// sends it. On failure, returns what git said.
pub async fn probe(url: &str, token: Option<&Token>) -> Result<(), String> {
    let output = upstream_output(Path::new("."), &["ls-remote", url, "HEAD"], url, token)
        .await
        .map_err(|e| format!("{e:#}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(git_message(&String::from_utf8_lossy(&output.stderr)))
}

/// What git said about a failure, for people: the server's `remote:` lines
/// if it sent any, otherwise git's own last error.
fn git_message(stderr: &str) -> String {
    let remote: Vec<&str> = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("remote:"))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if !remote.is_empty() {
        return remote.join(" ");
    }
    let last = stderr.lines().map(str::trim).rfind(|line| !line.is_empty());
    match last {
        Some(line) => ["fatal: ", "error: "]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix))
            .unwrap_or(line)
            .to_owned(),
        None => "git ls-remote failed without saying why".to_owned(),
    }
}

/// Initializes a bare repo next to the archive path and moves it into place,
/// so a crash never leaves a half-created archive.
async fn create_if_missing(archive: &Path) -> anyhow::Result<()> {
    if archive.exists() {
        return Ok(());
    }
    let parent = archive.parent().expect("archive path has a parent");
    std::fs::create_dir_all(parent)
        .with_context(|| format!("couldn't create {}", parent.display()))?;
    let new = tempfile::Builder::new()
        .prefix(".new-")
        .tempdir_in(parent)
        .with_context(|| format!("couldn't create a directory in {}", parent.display()))?;
    git(new.path(), &["init", "--quiet", "--bare"]).await?;
    std::fs::rename(new.path(), archive)
        .with_context(|| format!("couldn't create archive {}", archive.display()))?;
    let _ = new.keep();
    Ok(())
}

/// The branch upstream's `HEAD` points at, or `None` for an upstream with no
/// commits, which doesn't advertise one.
async fn upstream_default_branch(
    url: &str,
    token: Option<&Token>,
) -> anyhow::Result<Option<String>> {
    let args = ["ls-remote", "--symref", url, "HEAD"];
    let output = git_upstream(Path::new("."), &args, url, token).await?;
    let branch = output.lines().find_map(|line| {
        let (target, name) = line.strip_prefix("ref: ")?.split_once('\t')?;
        (name == "HEAD").then_some(target)
    });
    match branch {
        Some(target) => match target.strip_prefix("refs/heads/") {
            Some(branch) if !branch.is_empty() => Ok(Some(branch.to_owned())),
            _ => bail!("upstream HEAD points at {target}, not a branch"),
        },
        None if output.trim().is_empty() => Ok(None),
        None => bail!("upstream doesn't say which branch its HEAD points at"),
    }
}

/// A UTC time that is safe in a ref name: `20261116T120000Z`.
fn ref_timestamp(time: SystemTime) -> String {
    humantime::format_rfc3339_seconds(time)
        .to_string()
        .replace(['-', ':'], "")
}

async fn clear_staging(archive: &Path) -> anyhow::Result<()> {
    let leftovers = list_refs(archive, &[STAGING]).await?;
    let transaction: String = leftovers
        .iter()
        .map(|(name, sha)| format!("delete {name} {sha}\n"))
        .collect();
    git_with_input(archive, &["update-ref", "--stdin"], &transaction).await?;
    Ok(())
}

async fn list_refs(archive: &Path, prefixes: &[&str]) -> anyhow::Result<BTreeMap<String, String>> {
    let mut args = vec!["for-each-ref", "--format=%(refname) %(objectname)"];
    args.extend(prefixes);
    let output = git(archive, &args).await?;
    Ok(output
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(name, sha)| (name.to_owned(), sha.to_owned()))
        .collect())
}

async fn is_ancestor(archive: &Path, old: &str, new: &str) -> anyhow::Result<bool> {
    let output = command(archive, &["merge-base", "--is-ancestor", old, new])
        .output()
        .await
        .context("couldn't run git")?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "git merge-base failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    }
}

async fn rev_parse(archive: &Path, rev: &str) -> anyhow::Result<Option<String>> {
    let output = command(archive, &["rev-parse", "--verify", "--quiet", rev])
        .output()
        .await
        .context("couldn't run git")?;
    Ok(output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned()))
}

async fn git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    git_with_input(dir, args, "").await
}

async fn git_with_input(dir: &Path, args: &[&str], input: &str) -> anyhow::Result<String> {
    run_git(command(dir, args), dir, args, input).await
}

/// Runs a git command that talks to the upstream at `url`, sending it the
/// token if there is one. Its failure is a [`FetchError`], classified from
/// git's stderr.
async fn git_upstream(
    dir: &Path,
    args: &[&str],
    url: &str,
    token: Option<&Token>,
) -> anyhow::Result<String> {
    let output = upstream_output(dir, args, url, token).await?;
    if !output.status.success() {
        return Err(classify(&String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Runs a git command that talks to the upstream at `url`, sending it the
/// token if there is one, and returns what git printed and how it exited.
///
/// The token reaches git only through its environment, set through git's
/// `GIT_CONFIG_*` variables, so it never shows up in process arguments and is
/// never written to the archive's config. It goes out with the first request,
/// which is what lifts anonymous rate limits, but with redirects off, because
/// git would send it on to wherever upstream redirects. If upstream does
/// redirect, as hosts do for moved repos, the command runs again following
/// redirects, with the token offered only by a credential helper scoped to
/// `url`'s scheme, host and port: git then hands it over only if that host
/// answers 401, and never to another host.
async fn upstream_output(
    dir: &Path,
    args: &[&str],
    url: &str,
    token: Option<&Token>,
) -> anyhow::Result<Output> {
    let run =
        |mut command: Command| async move { command.output().await.context("couldn't run git") };
    let Some(token) = token else {
        return run(command(dir, args)).await;
    };
    let mut direct = command(dir, args);
    with_config(
        &mut direct,
        &[
            ("http.followRedirects", "false"),
            (
                "http.extraHeader",
                &format!("Authorization: {}", token.basic_auth()),
            ),
        ],
    );
    let output = run(direct).await?;
    let redirected = !output.status.success()
        && String::from_utf8_lossy(&output.stderr).contains("The requested URL returned error: 30");
    if !redirected {
        return Ok(output);
    }

    let origin = url::Url::parse(url)
        .with_context(|| format!("{url:?} is not a valid URL"))?
        .origin()
        .ascii_serialization();
    let mut following = command(dir, args);
    with_config(
        &mut following,
        &[(&format!("credential.{origin}.helper"), CREDENTIAL_HELPER)],
    );
    following.env(TOKEN_VARIABLE, token.reveal());
    run(following).await
}

/// Sets git config for one command through the environment.
fn with_config(command: &mut Command, settings: &[(&str, &str)]) {
    command.env("GIT_CONFIG_COUNT", settings.len().to_string());
    for (i, (key, value)) in settings.iter().enumerate() {
        command
            .env(format!("GIT_CONFIG_KEY_{i}"), key)
            .env(format!("GIT_CONFIG_VALUE_{i}"), value);
    }
}

/// Where the credential helper finds the token.
const TOKEN_VARIABLE: &str = "ARCHIVER_UPSTREAM_TOKEN";

/// Answers git's `get` requests with the token as the password, under the
/// user name GitHub expects for tokens (GitLab and Gitea accept any), and
/// ignores `store` and `erase`, so nothing is ever saved.
const CREDENTIAL_HELPER: &str = "!f() { test \"$1\" = get || return 0; \
     echo username=x-access-token; echo \"password=$ARCHIVER_UPSTREAM_TOKEN\"; }; f";

async fn run_git(
    mut command: Command,
    dir: &Path,
    args: &[&str],
    input: &str,
) -> anyhow::Result<String> {
    let mut child = command
        .stdin(Stdio::piped())
        .spawn()
        .context("couldn't run git")?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    stdin
        .write_all(input.as_bytes())
        .await
        .with_context(|| format!("couldn't send input to git {}", args[0]))?;
    drop(stdin);
    let output = child.wait_with_output().await.context("couldn't run git")?;
    if !output.status.success() {
        bail!(
            "git {} in {} failed: {}",
            args[0],
            dir.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A git command isolated from the user's and system's git config and from
/// any `GIT_*` variables in our environment, which never prompts.
pub(crate) fn command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    // Git asks this program for credentials on a 401 even when terminal
    // prompts are off.
    command.env_remove("SSH_ASKPASS");
    command
        .arg("-C")
        .arg(dir)
        .args(args.iter().map(OsStr::new))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ALLOW_PROTOCOL", "http:https")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}

/// Collects the archives under `dir`, which is `prefix` below `repos/`:
/// directories named `*.git`. Identities never put one inside another, so
/// the search stops at each.
fn find_archives(dir: &Path, prefix: &str, found: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = format!("{prefix}{name}");
        match path.strip_suffix(".git") {
            Some(archive) if !prefix.is_empty() => found.push(archive.to_owned()),
            _ if name.starts_with(".new-") || name.starts_with(".delete-") => {}
            _ => find_archives(&entry.path(), &format!("{path}/"), found)?,
        }
    }
    Ok(())
}

fn dir_size(dir: &Path) -> std::io::Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        total += if metadata.is_dir() {
            dir_size(&entry.path())?
        } else {
            metadata.len()
        };
    }
    Ok(total)
}
