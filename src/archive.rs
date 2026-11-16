//! Archives: one bare git repo per repo identity at `repos/<host>/<path>.git`,
//! created and updated only through the `git` CLI (ADR-0001).
//!
//! A fetch works in two steps. It first brings upstream's default branch and
//! tags into a staging namespace inside the archive, then moves the archived
//! refs to the staged state in one transaction and clears staging. A fetch
//! interrupted in between leaves only staging refs behind, which the next
//! fetch clears first.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, bail};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::identity::RepoId;

const STAGING: &str = "refs/staging/";

pub struct Archives {
    root: PathBuf,
}

/// Facts read from an archive.
#[derive(Debug, Clone, Default)]
pub struct Inspection {
    /// Only set once the archive holds that branch.
    pub default_branch: Option<String>,
    pub tip: Option<String>,
}

impl Archives {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("repos"),
        }
    }

    pub fn path(&self, id: &RepoId) -> PathBuf {
        self.root.join(id.archive_path())
    }

    /// Clones the repo into its archive, or updates the archive from
    /// upstream. Only adds history: archived refs move only by fast-forward,
    /// and refs upstream deleted or rewrote stay as they are.
    pub async fn fetch(&self, id: &RepoId, url: &str) -> anyhow::Result<()> {
        let archive = self.path(id);
        create_if_missing(&archive).await?;
        let default_branch = upstream_default_branch(url).await?;

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
        git(&archive, &args).await?;

        let staged = list_refs(&archive, &[STAGING]).await?;
        let archived = list_refs(&archive, &["refs/heads/", "refs/tags/"]).await?;
        let mut transaction = String::new();
        for (staged_ref, new) in &staged {
            let target = format!("refs/{}", &staged_ref[STAGING.len()..]);
            match archived.get(&target) {
                None => transaction.push_str(&format!("create {target} {new}\n")),
                Some(old) if old == new => {}
                Some(old)
                    if target.starts_with("refs/heads/")
                        && is_ancestor(&archive, old, new).await? =>
                {
                    transaction.push_str(&format!("update {target} {new} {old}\n"));
                }
                Some(old) => tracing::warn!(
                    repo = %id,
                    r#ref = target,
                    archived = old,
                    upstream = new,
                    "upstream rewrote a ref; keeping the archived one",
                ),
            }
            transaction.push_str(&format!("delete {staged_ref} {new}\n"));
        }
        git_with_input(&archive, &["update-ref", "--stdin"], &transaction).await?;

        if let Some(branch) = &default_branch {
            git(
                &archive,
                &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
            )
            .await?;
        }
        Ok(())
    }

    /// Reads the default branch and its tip from the archive, if there is one.
    pub async fn inspect(&self, id: &RepoId) -> anyhow::Result<Inspection> {
        let archive = self.path(id);
        if !archive.exists() {
            return Ok(Inspection::default());
        }
        let head = git(&archive, &["symbolic-ref", "HEAD"]).await?;
        let head = head.trim();
        let Some(tip) = rev_parse(&archive, head).await? else {
            return Ok(Inspection::default());
        };
        Ok(Inspection {
            default_branch: head.strip_prefix("refs/heads/").map(str::to_owned),
            tip: Some(tip),
        })
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
async fn upstream_default_branch(url: &str) -> anyhow::Result<Option<String>> {
    let output = git(Path::new("."), &["ls-remote", "--symref", url, "HEAD"]).await?;
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
    let mut child = command(dir, args)
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
fn command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
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
