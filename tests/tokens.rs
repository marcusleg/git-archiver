//! Per-host tokens: sent to the hosts they're configured for, and never
//! anywhere else.

mod common;

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::forge::FakeForge;
use common::git::refs;
use common::{HOUR, TestApp, config_tracking};

const TOKEN: &str = "s3cr3t-token-4f9a2c";
const WRONG_TOKEN: &str = "wr0ng-token-7d1e5b";

/// A config tracking the given repo URLs, with a token for one host.
fn config_with_token(urls: &[&str], host: &str, token: &str) -> String {
    format!(
        "{}\n[hosts.{host:?}]\ntoken = {token:?}\n",
        config_tracking(urls)
    )
}

fn repo_id(forge: &FakeForge, path: &str) -> String {
    format!("http://{}/{path}", forge.host())
}

/// Fails if `token` appears in `text`.
fn assert_hidden(text: &str, token: &str, place: &str) {
    assert!(!text.contains(token), "token leaked into {place}: {text}");
}

/// Fails if `token` appears on the list page, in `/metrics` or in
/// `state.json`.
async fn assert_hidden_by_app(app: &TestApp, token: &str) {
    assert_hidden(&app.get("/").await.text(), token, "the list page");
    assert_hidden(&app.metrics().await.text, token, "/metrics");
    let state = std::fs::read_to_string(app.data_dir().join("state.json")).unwrap();
    assert_hidden(&state, token, "state.json");
}

/// Fails if `token`, or any `Authorization` header, appears in any file
/// under `dir`.
fn assert_hidden_in_files(dir: &Path, token: &str) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert_hidden_in_files(&path, token);
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let text = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
        let place = path.display().to_string();
        assert_hidden(&text, &token.to_ascii_lowercase(), &place);
        assert_hidden(&text, "authorization", &place);
        assert_hidden(&text, "extraheader", &place);
    }
}

#[tokio::test]
async fn a_repo_that_requires_a_token_is_fetched_with_the_configured_token() {
    let forge = FakeForge::start().await;
    forge.require_token(TOKEN);
    let upstream = forge.create_repo("private/repo");
    let tip = upstream.commit("first");
    let app =
        TestApp::with_config(&config_with_token(&[upstream.url()], &forge.host(), TOKEN)).await;
    let start = app.now();

    app.advance_and_settle(24 * HOUR).await;

    let archive = app.archive(&format!("{}/private/repo", forge.host()));
    assert_eq!(refs(&archive)["refs/heads/main"], tip);
    let metrics = app.metrics().await;
    assert_eq!(
        metrics.last_success(&repo_id(&forge, "private/repo")),
        Some(start + 24 * 3600)
    );

    assert_hidden_by_app(&app, TOKEN).await;
    assert_hidden_in_files(&app.data_dir().join("repos"), TOKEN);
}

#[tokio::test]
async fn without_the_token_a_repo_that_requires_one_isnt_fetched() {
    let forge = FakeForge::start().await;
    forge.require_token(TOKEN);
    let upstream = forge.create_repo("private/repo");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;

    app.advance_and_settle(24 * HOUR).await;

    let metrics = app.metrics().await;
    let id = repo_id(&forge, "private/repo");
    assert_eq!(metrics.last_success(&id), None);
    assert_eq!(metrics.repo_status(&id), Some("unavailable"));
    let state = std::fs::read_to_string(app.data_dir().join("state.json")).unwrap();
    assert!(state.contains("401"), "{state}");
    let archive = app.archive(&format!("{}/private/repo", forge.host()));
    assert!(!archive.exists() || refs(&archive).is_empty());
    assert!(forge.authorizations().is_empty());
}

#[tokio::test]
async fn repos_on_other_hosts_are_fetched_without_the_token() {
    let with_token = FakeForge::start().await;
    with_token.require_token(TOKEN);
    let other = FakeForge::start().await;
    let private = with_token.create_repo("private/repo");
    private.commit("first");
    let public = other.create_repo("public/repo");
    let public_tip = public.commit("first");
    let app = TestApp::with_config(&config_with_token(
        &[private.url(), public.url()],
        &with_token.host(),
        TOKEN,
    ))
    .await;

    app.advance_and_settle(12 * HOUR).await;
    app.advance_and_settle(12 * HOUR).await;

    let metrics = app.metrics().await;
    assert!(
        metrics
            .last_success(&repo_id(&with_token, "private/repo"))
            .is_some()
    );
    let archive = app.archive(&format!("{}/public/repo", other.host()));
    assert_eq!(refs(&archive)["refs/heads/main"], public_tip);
    assert!(!with_token.authorizations().is_empty());
    assert_eq!(other.authorizations(), Vec::<String>::new());
}

#[tokio::test]
async fn the_token_isnt_sent_to_another_host_upstream_redirects_to() {
    let with_token = FakeForge::start().await;
    let other = FakeForge::start().await;
    let moved = with_token.create_repo("old/name");
    let target = other.create_repo("new/name");
    let tip = target.commit("first");
    with_token.redirect("old/name", &target);
    let app = TestApp::with_config(&config_with_token(
        &[moved.url()],
        &with_token.host(),
        TOKEN,
    ))
    .await;

    app.advance_and_settle(24 * HOUR).await;

    assert_eq!(other.authorizations(), Vec::<String>::new());
    let archive = app.archive(&format!("{}/old/name", with_token.host()));
    assert_eq!(refs(&archive)["refs/heads/main"], tip);
}

#[tokio::test]
async fn a_repo_moved_within_its_host_is_still_fetched_with_the_token() {
    let forge = FakeForge::start().await;
    forge.require_token(TOKEN);
    let moved = forge.create_repo("old/name");
    let target = forge.create_repo("new/name");
    let tip = target.commit("first");
    forge.redirect("old/name", &target);
    let app = TestApp::with_config(&config_with_token(&[moved.url()], &forge.host(), TOKEN)).await;

    app.advance_and_settle(24 * HOUR).await;

    let archive = app.archive(&format!("{}/old/name", forge.host()));
    assert_eq!(refs(&archive)["refs/heads/main"], tip);
}

#[tokio::test]
async fn the_token_goes_out_with_the_first_request_so_anonymous_limits_never_apply() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("public/repo");
    upstream.commit("first");
    let app =
        TestApp::with_config(&config_with_token(&[upstream.url()], &forge.host(), TOKEN)).await;

    app.advance_and_settle(24 * HOUR).await;

    let metrics = app.metrics().await;
    assert!(
        metrics
            .last_success(&repo_id(&forge, "public/repo"))
            .is_some()
    );
    let authorizations = forge.authorizations();
    assert!(
        !authorizations.is_empty(),
        "upstream never asked, so it never got the token"
    );
}

#[tokio::test]
async fn a_token_applies_to_its_host_whatever_the_case() {
    let forge = FakeForge::start().await;
    forge.require_token(TOKEN);
    let upstream = forge.create_repo("private/repo");
    let tip = upstream.commit("first");
    let port = forge.host().rsplit_once(':').unwrap().1.to_owned();
    let url = format!("http://LocalHost:{port}/private/repo.git");
    let app = TestApp::with_config(&config_with_token(
        &[&url],
        &format!("LOCALHOST:{port}"),
        TOKEN,
    ))
    .await;

    app.advance_and_settle(24 * HOUR).await;

    let archive = app.archive(&format!("localhost:{port}/private/repo"));
    assert_eq!(refs(&archive)["refs/heads/main"], tip);
}

#[tokio::test]
async fn changing_the_token_in_the_config_takes_effect_from_the_next_fetch() {
    let forge = FakeForge::start().await;
    forge.require_token(TOKEN);
    let upstream = forge.create_repo("private/repo");
    upstream.commit("first");
    let id = repo_id(&forge, "private/repo");
    let app = TestApp::with_config(&config_with_token(
        &[upstream.url()],
        &forge.host(),
        WRONG_TOKEN,
    ))
    .await;
    let start = app.now();

    app.advance_and_settle(24 * HOUR).await;
    let metrics = app.metrics().await;
    assert_eq!(metrics.last_success(&id), None);
    assert_eq!(metrics.repo_status(&id), Some("unavailable"));
    assert_hidden_by_app(&app, WRONG_TOKEN).await;

    app.write_config(&config_with_token(&[upstream.url()], &forge.host(), TOKEN));
    app.advance_and_settle(24 * HOUR).await;

    assert_eq!(
        app.metrics().await.last_success(&id),
        Some(start + 48 * 3600)
    );
}

/// The command lines of every process running right now.
fn all_command_lines() -> Vec<String> {
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| std::fs::read(entry.ok()?.path().join("cmdline")).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).replace('\0', " "))
        .collect()
}

#[tokio::test]
async fn the_token_never_appears_in_the_arguments_of_git_commands() {
    let forge = FakeForge::start().await;
    forge.require_token(TOKEN);
    let upstream = forge.create_repo("private/repo");
    upstream.commit("first");
    forge.hang("private/repo");
    let app =
        TestApp::with_config(&config_with_token(&[upstream.url()], &forge.host(), TOKEN)).await;

    app.advance(24 * HOUR);
    forge.wait_for_hung_request().await;

    let command_lines = all_command_lines();
    let git_commands: Vec<&String> = command_lines
        .iter()
        .filter(|line| line.contains(&forge.host()))
        .collect();
    assert!(
        !git_commands.is_empty(),
        "git should be running against the forge: {command_lines:#?}"
    );
    for line in &command_lines {
        assert_hidden(line, TOKEN, "process arguments");
    }
    // Catches the token in encoded form too, as a header passed with `-c`.
    for line in git_commands {
        assert_hidden(
            &line.to_ascii_lowercase(),
            "authorization",
            "git's arguments",
        );
    }
}

/// Runs the real binary against `data_dir` until `done` holds, then returns
/// everything it logged.
async fn run_binary_until(data_dir: &Path, mut done: impl FnMut() -> bool) -> String {
    let log_path = data_dir.join("app.log");
    let log = std::fs::File::create(&log_path).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_git-archiver"))
        .arg("--data-dir")
        .arg(data_dir)
        .env("RUST_LOG", "trace")
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .map(KillOnDrop)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!("app exited with {status}");
        }
        assert!(Instant::now() < deadline, "app didn't get there in time");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    drop(child);
    std::fs::read_to_string(log_path).unwrap()
}

struct KillOnDrop(std::process::Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn the_token_never_appears_in_the_logs_of_good_or_failed_fetches() {
    let forge = FakeForge::start().await;
    forge.require_token(TOKEN);
    let upstream = forge.create_repo("private/repo");
    upstream.commit("first");
    let data_dir = tempfile::TempDir::new().unwrap();
    let listen = "listen = \"127.0.0.1:0\"\ninterval = \"1s\"\n";
    let write_config = |token: &str| {
        std::fs::write(
            data_dir.path().join("config.toml"),
            format!(
                "{listen}{}",
                config_with_token(&[upstream.url()], &forge.host(), token)
            ),
        )
        .unwrap();
    };
    let log_path = data_dir.path().join("app.log");
    let logged =
        |needle: &str| std::fs::read_to_string(&log_path).is_ok_and(|log| log.contains(needle));

    write_config(TOKEN);
    let log = run_binary_until(data_dir.path(), || logged("fetched")).await;
    assert!(log.contains("fetched"), "{log}");
    assert_hidden(&log, TOKEN, "the log");

    write_config(WRONG_TOKEN);
    let log = run_binary_until(data_dir.path(), || logged("unavailable")).await;
    assert!(log.contains("unavailable"), "{log}");
    assert_hidden(&log, WRONG_TOKEN, "the log");
    assert_hidden(&log.to_ascii_lowercase(), "authorization:", "the log");
}
