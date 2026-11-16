//! Rewrites upstream: what a fetch preserves before the archive takes the new
//! state, read back with plain `git`.

mod common;

use common::forge::FakeForge;
use common::git::{commit_of, git, refs};
use common::{HOUR, TestApp};

/// With one repo and the default 24h interval, the first fetch runs a day
/// after the test clock's start (2026-11-17T12:00:00Z) and every later fetch
/// another day on.
const SECOND_FETCH: &str = "20261118T120000Z";
const THIRD_FETCH: &str = "20261119T120000Z";

fn names(refs: &std::collections::BTreeMap<String, String>) -> Vec<&str> {
    refs.keys().map(String::as_str).collect()
}

#[tokio::test]
async fn a_force_push_preserves_the_old_tip_and_moves_the_branch() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    let old = upstream.commit("second");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));

    let new = upstream.force_push_new_history("rewritten");
    app.advance_and_settle(24 * HOUR).await;

    let refs = refs(&archive);
    let preserved = format!("refs/preserved/{SECOND_FETCH}/heads/main");
    assert_eq!(names(&refs), ["refs/heads/main", preserved.as_str()]);
    assert_eq!(refs["refs/heads/main"], new);
    assert_eq!(refs[&preserved], old);
    assert_eq!(git(&archive, &["symbolic-ref", "HEAD"]), "refs/heads/main");
}

#[tokio::test]
async fn a_moved_tag_preserves_its_old_target_and_a_deleted_tag_its_last_one() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let first = upstream.commit("first");
    upstream.tag("moved");
    upstream.tag("deleted");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    let before = refs(&archive);

    let second = upstream.commit("second");
    upstream.move_tag("moved");
    upstream.delete_tag("deleted");
    app.advance_and_settle(24 * HOUR).await;

    let refs = refs(&archive);
    let moved = format!("refs/preserved/{SECOND_FETCH}/tags/moved");
    let deleted = format!("refs/preserved/{SECOND_FETCH}/tags/deleted");
    assert_eq!(
        names(&refs),
        [
            "refs/heads/main",
            deleted.as_str(),
            moved.as_str(),
            "refs/tags/moved",
        ]
    );
    assert_eq!(refs[&moved], before["refs/tags/moved"]);
    assert_eq!(refs[&deleted], before["refs/tags/deleted"]);
    assert_eq!(commit_of(&archive, &moved), first);
    assert_eq!(commit_of(&archive, &deleted), first);
    assert_eq!(commit_of(&archive, "refs/tags/moved"), second);
    assert_eq!(refs["refs/heads/main"], second);
}

#[tokio::test]
async fn a_default_branch_switch_preserves_the_old_branch_and_follows_the_new_one() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo_with_default_branch("foo/bar", "master");
    let old = upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));

    upstream.switch_default_branch("main");
    let new = upstream.commit("second");
    app.advance_and_settle(24 * HOUR).await;

    let refs = refs(&archive);
    let preserved = format!("refs/preserved/{SECOND_FETCH}/heads/master");
    assert_eq!(names(&refs), ["refs/heads/main", preserved.as_str()]);
    assert_eq!(refs[&preserved], old);
    assert_eq!(refs["refs/heads/main"], new);
    assert_eq!(git(&archive, &["symbolic-ref", "HEAD"]), "refs/heads/main");
}

#[tokio::test]
async fn new_commits_and_new_tags_preserve_nothing() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    upstream.tag("v1");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));

    upstream.commit("second");
    upstream.tag("v2");
    app.advance_and_settle(24 * HOUR).await;
    app.advance_and_settle(24 * HOUR).await;

    assert_eq!(
        names(&refs(&archive)),
        ["refs/heads/main", "refs/tags/v1", "refs/tags/v2"]
    );
}

#[tokio::test]
async fn later_fetches_never_remove_or_change_preserved_refs() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    upstream.tag("v1");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    upstream.force_push_new_history("rewritten once");
    upstream.move_tag("v1");
    app.advance_and_settle(24 * HOUR).await;
    let preserved_once: Vec<(String, String)> = refs(&archive)
        .into_iter()
        .filter(|(name, _)| name.starts_with("refs/preserved/"))
        .collect();
    assert_eq!(preserved_once.len(), 2);

    upstream.force_push_new_history("rewritten twice");
    upstream.delete_tag("v1");
    app.advance_and_settle(24 * HOUR).await;
    app.advance_and_settle(24 * HOUR).await;

    let refs = refs(&archive);
    for (name, sha) in &preserved_once {
        assert_eq!(refs.get(name), Some(sha), "{name}");
    }
    let third = format!("refs/preserved/{THIRD_FETCH}/");
    let preserved: Vec<&str> = names(&refs)
        .into_iter()
        .filter(|name| name.starts_with("refs/preserved/"))
        .collect();
    assert_eq!(
        preserved,
        [
            format!("refs/preserved/{SECOND_FETCH}/heads/main"),
            format!("refs/preserved/{SECOND_FETCH}/tags/v1"),
            format!("{third}heads/main"),
            format!("{third}tags/v1"),
        ]
    );
}

#[tokio::test]
async fn a_fetch_interrupted_after_staging_loses_nothing_when_fetched_again() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let old_tip = upstream.commit("first");
    upstream.tag("moved");
    upstream.tag("deleted");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    let before = refs(&archive);

    // A fetch that died after bringing upstream's rewritten state into
    // staging, before applying it.
    upstream.force_push_new_history("rewritten");
    upstream.move_tag("moved");
    upstream.delete_tag("deleted");
    git(
        &archive,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            upstream.served_path(),
            "+refs/heads/main:refs/staging/heads/main",
            "+refs/tags/*:refs/staging/tags/*",
        ],
    );
    let new_tip = upstream.commit("after the interruption");
    app.advance_and_settle(24 * HOUR).await;

    let refs = refs(&archive);
    let preserved = format!("refs/preserved/{SECOND_FETCH}/");
    assert_eq!(
        names(&refs),
        [
            "refs/heads/main",
            format!("{preserved}heads/main").as_str(),
            format!("{preserved}tags/deleted").as_str(),
            format!("{preserved}tags/moved").as_str(),
            "refs/tags/moved",
        ]
    );
    assert_eq!(refs[&format!("{preserved}heads/main")], old_tip);
    assert_eq!(
        refs[&format!("{preserved}tags/moved")],
        before["refs/tags/moved"]
    );
    assert_eq!(
        refs[&format!("{preserved}tags/deleted")],
        before["refs/tags/deleted"]
    );
    assert_eq!(refs["refs/heads/main"], new_tip);
}

#[tokio::test]
async fn a_default_branch_switch_interrupted_before_head_moved_completes_on_the_next_fetch() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo_with_default_branch("foo/bar", "master");
    let old = upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));

    // A fetch that died with the new default branch staged and `HEAD`
    // already pointing at it, the old branch not yet preserved.
    upstream.switch_default_branch("main");
    let new = upstream.commit("second");
    git(
        &archive,
        &[
            "fetch",
            "--quiet",
            "--no-tags",
            upstream.served_path(),
            "+refs/heads/main:refs/staging/heads/main",
        ],
    );
    git(&archive, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    app.advance_and_settle(24 * HOUR).await;

    let refs = refs(&archive);
    let preserved = format!("refs/preserved/{SECOND_FETCH}/heads/master");
    assert_eq!(names(&refs), ["refs/heads/main", preserved.as_str()]);
    assert_eq!(refs[&preserved], old);
    assert_eq!(refs["refs/heads/main"], new);
    assert_eq!(git(&archive, &["symbolic-ref", "HEAD"]), "refs/heads/main");
}

#[tokio::test]
async fn every_rewrite_logs_one_line_with_the_repo_and_the_preserved_refs() {
    let logs = common::logs::capture();
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    upstream.tag("v1");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;

    upstream.force_push_new_history("rewritten");
    upstream.move_tag("v1");
    app.advance_and_settle(24 * HOUR).await;
    upstream.commit("ordinary");
    app.advance_and_settle(24 * HOUR).await;

    let repo = format!("http://{}/foo/bar", forge.host());
    let lines = logs.matching("refs/preserved/");
    assert_eq!(lines.len(), 1, "{lines:#?}");
    let line = &lines[0];
    assert!(line.contains(&format!("repo={repo}")), "{line}");
    for name in [
        format!("refs/preserved/{SECOND_FETCH}/heads/main"),
        format!("refs/preserved/{SECOND_FETCH}/tags/v1"),
    ] {
        assert!(line.contains(&name), "{line}");
    }
}
