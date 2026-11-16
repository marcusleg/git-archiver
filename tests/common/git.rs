//! Runs plain `git` the way a person inspecting an archive would, isolated
//! from the user's own git config.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Output};

/// A git command in `dir`, isolated from the user's config, which never
/// prompts.
fn command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com");
    command
}

/// Asserts git succeeded and returns its trimmed output.
fn stdout_of(dir: &Path, args: &[&str], output: Output) -> String {
    assert!(
        output.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = command(dir, args).output().expect("git runs");
    stdout_of(dir, args, output)
}

/// Runs git without asserting success, off the async runtime so it can talk
/// to the app under test, which runs on the same runtime.
pub async fn try_git(dir: &Path, args: &[&str]) -> Output {
    let mut command = command(dir, args);
    tokio::task::spawn_blocking(move || command.output().expect("git runs"))
        .await
        .unwrap()
}

/// Like `git`, but usable against the app under test.
pub async fn git_async(dir: &Path, args: &[&str]) -> String {
    let output = try_git(dir, args).await;
    stdout_of(dir, args, output)
}

/// Every ref in a repo, by name, with the object it points at.
pub fn refs(repo: &Path) -> BTreeMap<String, String> {
    git(repo, &["for-each-ref", "--format=%(refname) %(objectname)"])
        .lines()
        .map(|line| {
            let (name, sha) = line.split_once(' ').unwrap();
            (name.to_owned(), sha.to_owned())
        })
        .collect()
}

/// The commit a ref (or a tag, peeled) points at.
pub fn commit_of(repo: &Path, rev: &str) -> String {
    git(repo, &["rev-parse", &format!("{rev}^{{commit}}")])
}

pub fn has_object(repo: &Path, sha: &str) -> bool {
    command(repo, &["cat-file", "-e", sha])
        .output()
        .unwrap()
        .status
        .success()
}
