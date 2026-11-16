//! Getting archives out: cloning them read-only over git's smart HTTP
//! protocol from `/git/<host>/<path>.git`.

mod common;

use common::forge::FakeForge;
use common::git::{commit_of, git, git_async, refs, try_git};
use common::{HOUR, TestApp};
use scraper::{Html, Selector};
use tempfile::TempDir;

/// The app's git endpoint URL for a repo on the forge.
fn endpoint(app: &TestApp, forge: &FakeForge, path: &str) -> String {
    format!("http://{}/git/{}/{path}.git", app.addr(), forge.host())
}

#[tokio::test]
async fn a_clone_from_the_endpoint_yields_the_default_branch_and_tags() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo_with_default_branch("foo/bar", "trunk");
    let first = upstream.commit("first");
    upstream.tag("v1");
    let second = upstream.commit("second");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;

    let out = TempDir::new().unwrap();
    git_async(
        out.path(),
        &["clone", "-q", &endpoint(&app, &forge, "foo/bar"), "clone"],
    )
    .await;

    let clone = out.path().join("clone");
    assert_eq!(git(&clone, &["symbolic-ref", "--short", "HEAD"]), "trunk");
    assert_eq!(git(&clone, &["rev-parse", "HEAD"]), second);
    assert_eq!(commit_of(&clone, "refs/tags/v1"), first);
}

#[tokio::test]
async fn ls_remote_on_the_endpoint_shows_preserved_refs_but_not_staging() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let first = upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    let preserved = "refs/preserved/20261116T120000Z/heads/main";
    git(&archive, &["update-ref", preserved, &first]);
    git(&archive, &["update-ref", "refs/staging/heads/main", &first]);

    let out = TempDir::new().unwrap();
    let listing = git_async(
        out.path(),
        &["ls-remote", &endpoint(&app, &forge, "foo/bar")],
    )
    .await;

    assert!(
        listing.contains(&format!("{first}\t{preserved}")),
        "{listing}"
    );
    assert!(listing.contains("refs/heads/main"), "{listing}");
    assert!(!listing.contains("refs/staging/"), "{listing}");
}

#[tokio::test]
async fn a_push_to_the_endpoint_is_refused_and_leaves_the_archive_unchanged() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    let before = refs(&archive);

    let out = TempDir::new().unwrap();
    let url = endpoint(&app, &forge, "foo/bar");
    git_async(out.path(), &["clone", "-q", &url, "clone"]).await;
    let clone = out.path().join("clone");
    std::fs::write(clone.join("file.txt"), "pushed").unwrap();
    git(&clone, &["commit", "-q", "-am", "pushed"]);
    let push = try_git(&clone, &["push", "origin", "HEAD:refs/heads/main"]).await;
    let new_branch = try_git(&clone, &["push", "origin", "HEAD:refs/heads/other"]).await;

    for refused in [&push, &new_branch] {
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(!refused.status.success());
        assert!(stderr.contains("403"), "{stderr}");
    }
    assert_eq!(refs(&archive), before);
}

#[tokio::test]
async fn unknown_repos_traversal_and_non_archive_directories_get_a_404() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let host = forge.host();
    let repos = app.data_dir().join("repos");
    // A plain directory named like an archive, a bare repo outside `repos/`
    // reached through a symlink, and a bare repo directly under `repos/`.
    std::fs::create_dir_all(repos.join(&host).join("plain.git")).unwrap();
    let outside = TempDir::new().unwrap();
    git(outside.path(), &["init", "-q", "--bare", "elsewhere.git"]);
    std::os::unix::fs::symlink(
        outside.path().join("elsewhere.git"),
        repos.join(&host).join("link.git"),
    )
    .unwrap();
    git(&repos, &["init", "-q", "--bare", "top.git"]);
    let refs = "info/refs?service=git-upload-pack";

    assert_eq!(
        app.raw_get_status(&format!("/git/{host}/foo/bar.git/{refs}"))
            .await,
        200,
        "the real archive is served"
    );
    for target in [
        format!("/git/{host}/foo/nope.git/{refs}"),
        format!("/git/{host}/foo/bar/{refs}"),
        format!("/git/{host}/plain.git/{refs}"),
        format!("/git/{host}/link.git/{refs}"),
        format!("/git/top.git/{refs}"),
        format!("/git/{host}/foo/bar.git"),
        "/git/".to_owned(),
        "/git/../config.toml".to_owned(),
        "/git/../state.json".to_owned(),
        format!("/git/{host}/../{host}/foo/bar.git/{refs}"),
        format!("/git/{host}/./foo/bar.git/{refs}"),
        format!("/git/{host}/foo/bar.git/../../../../config.toml"),
        format!("/git/{host}/foo/bar.git/../bar.git/{refs}"),
        "/git/%2e%2e/config.toml".to_owned(),
        "/git/%2E%2E/%2e%2e/config.toml".to_owned(),
        "/git/..%2fconfig.toml".to_owned(),
        format!("/git/{host}/foo%2fbar.git/{refs}"),
        format!("/git/{host}/foo/bar.git/..%2f..%2f..%2f..%2fconfig.toml"),
        format!("/git/{host}/foo/bar.git/%2e%2e/%2e%2e/%2e%2e/%2e%2e/config.toml"),
        format!("/git/{host}/foo/bar.git/config"),
    ] {
        assert_eq!(app.raw_get_status(&target).await, 404, "{target}");
    }
}

#[tokio::test]
async fn each_archived_repos_card_shows_a_clone_line_that_works_as_a_clone_command() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let tip = upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url(), "https://github.com/a/b"]).await;
    app.advance_and_settle(12 * HOUR).await;
    assert!(!app.archive("github.com/a/b").exists());

    let html = Html::parse_document(&app.get("/").await.text());
    let lines: Vec<String> = html
        .select(&Selector::parse("main article code.clone").unwrap())
        .map(|code| code.text().collect())
        .collect();

    assert_eq!(
        lines,
        [format!("git clone {}", endpoint(&app, &forge, "foo/bar"))],
        "only the repo with an archive has a clone line"
    );
    let out = TempDir::new().unwrap();
    let args: Vec<&str> = lines[0].split_whitespace().skip(1).collect();
    git_async(out.path(), &args).await;
    assert_eq!(git(&out.path().join("bar"), &["rev-parse", "HEAD"]), tip);
}

#[tokio::test]
async fn an_untracked_archive_can_still_be_cloned() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let tip = upstream.commit("first");
    let mut app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    app.write_config("repos = []\n");
    app.restart().await;

    let out = TempDir::new().unwrap();
    let listing = git_async(
        out.path(),
        &["ls-remote", &endpoint(&app, &forge, "foo/bar"), "HEAD"],
    )
    .await;

    assert_eq!(listing, format!("{tip}\tHEAD"));
}

#[tokio::test]
async fn a_clone_is_served_while_a_fetch_of_the_same_archive_is_in_progress() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let tip = upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    upstream.commit("second");
    forge.hang("foo/bar");
    app.advance(24 * HOUR);
    forge.wait_for_hung_request().await;

    let out = TempDir::new().unwrap();
    git_async(
        out.path(),
        &["clone", "-q", &endpoint(&app, &forge, "foo/bar"), "clone"],
    )
    .await;

    assert_eq!(git(&out.path().join("clone"), &["rev-parse", "HEAD"]), tip);
}
