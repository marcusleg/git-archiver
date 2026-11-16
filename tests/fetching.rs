//! What a fetch puts into an archive, read back with plain `git`.

mod common;

use common::forge::FakeForge;
use common::git::{commit_of, git, has_object, refs};
use common::{HOUR, TestApp};

#[tokio::test]
async fn a_fetch_archives_the_default_branch_and_all_tags_as_a_bare_repo() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo_with_default_branch("foo/bar", "trunk");
    let first = upstream.commit("first");
    upstream.tag("v1");
    let second = upstream.commit("second");
    upstream.tag("v2");
    upstream.push_branch("feature");
    let app = TestApp::tracking(&[upstream.url()]).await;

    app.advance_and_settle(24 * HOUR).await;

    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    assert_eq!(
        git(&archive, &["rev-parse", "--is-bare-repository"]),
        "true"
    );
    let refs = refs(&archive);
    let names: Vec<&str> = refs.keys().map(String::as_str).collect();
    assert_eq!(names, ["refs/heads/trunk", "refs/tags/v1", "refs/tags/v2"]);
    assert_eq!(refs["refs/heads/trunk"], second);
    assert_eq!(commit_of(&archive, "refs/tags/v1"), first);
    assert_eq!(commit_of(&archive, "refs/tags/v2"), second);
    assert_eq!(git(&archive, &["symbolic-ref", "HEAD"]), "refs/heads/trunk");
}

#[tokio::test]
async fn new_upstream_commits_and_tags_arrive_with_the_next_fetch() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let first = upstream.commit("first");
    upstream.tag("v1");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    assert_eq!(refs(&archive)["refs/heads/main"], first);

    let second = upstream.commit("second");
    let third = upstream.commit("third");
    upstream.tag("v2");
    app.advance_and_settle(24 * HOUR).await;

    let refs = refs(&archive);
    let names: Vec<&str> = refs.keys().map(String::as_str).collect();
    assert_eq!(names, ["refs/heads/main", "refs/tags/v1", "refs/tags/v2"]);
    assert_eq!(refs["refs/heads/main"], third);
    assert_eq!(commit_of(&archive, "refs/tags/v1"), first);
    assert_eq!(commit_of(&archive, "refs/tags/v2"), third);
    assert!(git(&archive, &["merge-base", "--is-ancestor", &second, &third]).is_empty());
}

#[tokio::test]
async fn a_non_fast_forward_upstream_change_never_removes_objects_or_refs() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let first = upstream.commit("first");
    upstream.tag("v1");
    upstream.tag("v2");
    let tip = upstream.commit("second");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    let before = refs(&archive);

    upstream.force_push_new_history("rewritten");
    upstream.move_tag("v1");
    upstream.delete_tag("v2");
    app.advance_and_settle(24 * HOUR).await;

    assert_eq!(refs(&archive), before);
    assert!(has_object(&archive, &first));
    assert!(has_object(&archive, &tip));
    assert_eq!(commit_of(&archive, "refs/tags/v1"), first);
    assert_eq!(commit_of(&archive, "refs/tags/v2"), first);
}

#[tokio::test]
async fn urls_with_the_same_repo_identity_share_one_archive() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let tip = upstream.commit("first");
    let with_slash = format!("{}/", upstream.url());
    let app = TestApp::tracking(&[&with_slash, upstream.url()]).await;
    let start = app.now();

    app.advance_and_settle(24 * HOUR).await;

    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    assert_eq!(refs(&archive)["refs/heads/main"], tip);
    let metrics = app.metrics().await;
    let id = format!("http://{}/foo/bar", forge.host());
    assert_eq!(
        metrics.last_success(&id),
        Some(start + 24 * 3600),
        "one repo, so a 24h gap"
    );
    let page = app.get("/").await.text();
    assert_eq!(page.matches("<article").count(), 1, "{page}");
}
