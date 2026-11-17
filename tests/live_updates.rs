//! The event stream at `/events` that tells open pages when what they show
//! changed.

mod common;

use std::time::Duration;

use common::forge::{FakeForge, Upstream};
use common::{HOUR, MINUTE, TestApp, config_tracking};

/// A forge with `a/one` holding a commit, and the app tracking it.
async fn tracking_one() -> (FakeForge, Upstream, TestApp) {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("a/one");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    (forge, upstream, app)
}

#[tokio::test]
async fn the_stream_is_server_sent_events_that_proxies_pass_on_unbuffered() {
    let (_forge, _upstream, app) = tracking_one().await;
    let events = app.events().await;
    assert_eq!(events.header("content-type"), Some("text/event-stream"));
    assert_eq!(events.header("cache-control"), Some("no-cache"));
    assert_eq!(events.header("x-accel-buffering"), Some("no"));
}

#[tokio::test]
async fn a_completed_fetch_is_announced() {
    let (_forge, _upstream, app) = tracking_one().await;
    let mut events = app.events().await;
    app.advance_and_settle(24 * HOUR).await;
    events.changed().await;
}

#[tokio::test]
async fn adding_a_repo_through_the_form_is_announced() {
    let (forge, _upstream, app) = tracking_one().await;
    let second = forge.create_repo("a/two");
    second.commit("first");
    let mut events = app.events().await;
    let page = app.add_repo(second.url()).await;
    assert_eq!((page.status, page.path.as_str()), (200, "/"));
    events.changed().await;
}

/// Untracks `a/one` from its page, choosing `choice` in the dialog, and
/// checks an event follows.
async fn untracking_is_announced(choice: &str) {
    let (forge, _upstream, app) = tracking_one().await;
    app.advance_and_settle(24 * HOUR).await;
    let mut events = app.events().await;
    let page = app
        .untrack_from_page(&format!("/repos/{}/a/one", forge.host()), choice)
        .await;
    assert_eq!((page.status, page.path.as_str()), (200, "/"));
    events.changed().await;
}

#[tokio::test]
async fn untracking_and_keeping_the_archive_is_announced() {
    untracking_is_announced("Keep archive").await;
}

#[tokio::test]
async fn untracking_and_deleting_the_archive_is_announced() {
    untracking_is_announced("Delete archive").await;
}

#[tokio::test]
async fn deleting_the_archive_of_a_first_fetch_stopped_by_untracking_is_announced() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("a/one");
    upstream.commit("first");
    forge.hang_transfer("a/one");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance(24 * HOUR);
    forge.wait_for_hung_request().await;
    let archive = app.archive(&format!("{}/a/one", forge.host()));
    assert!(archive.exists(), "the first clone has made the archive");

    let mut events = app.events().await;
    let page = app
        .untrack_from_page(&format!("/repos/{}/a/one", forge.host()), "Keep archive")
        .await;
    assert_eq!((page.status, page.path.as_str()), (200, "/"));
    // One event for the untrack, which a page may act on before the
    // scheduler deletes the half-made archive, and one for the deletion.
    // They don't merge into one: on the test's single-threaded runtime the
    // stream sends the first while the deletion waits on a blocking thread.
    events.changed().await;
    events.changed().await;

    assert!(!archive.exists(), "a half-made archive isn't kept");
    assert_eq!(app.list_page().await.untracked(), []);
}

#[tokio::test]
async fn a_config_edit_is_announced_once_picked_up() {
    let (forge, upstream, app) = tracking_one().await;
    let second = forge.create_repo("a/two");
    let mut events = app.events().await;
    app.write_config(&config_tracking(&[upstream.url(), second.url()]));
    events.stays_quiet_for(Duration::from_millis(200)).await;

    app.advance_and_settle(MINUTE).await;
    events.changed().await;
}

#[tokio::test]
async fn a_config_error_is_announced_as_it_appears_changes_and_clears() {
    let (_forge, upstream, app) = tracking_one().await;
    let mut events = app.events().await;

    app.write_config("repos = [");
    app.advance_and_settle(MINUTE).await;
    events.changed().await;

    app.write_config("repos = 3\n");
    app.advance_and_settle(MINUTE).await;
    events.changed().await;

    app.write_config(&config_tracking(&[upstream.url()]));
    app.advance_and_settle(MINUTE).await;
    events.changed().await;
}

#[tokio::test]
async fn a_reload_that_changes_nothing_shown_is_not_announced() {
    let (_forge, upstream, app) = tracking_one().await;
    let mut events = app.events().await;

    // Rewritten, but the same repos.
    app.write_config(&format!(
        "# Reformatted by hand.\nrepos = [{:?}]\n",
        upstream.url()
    ));
    app.advance_and_settle(MINUTE).await;
    app.advance_and_settle(MINUTE).await;
    events.stays_quiet_for(Duration::from_millis(500)).await;

    // A broken config, then the same breakage again.
    app.write_config("repos = [");
    app.advance_and_settle(MINUTE).await;
    events.changed().await;
    app.advance_and_settle(MINUTE).await;
    events.stays_quiet_for(Duration::from_millis(500)).await;
}

#[tokio::test]
async fn shutting_down_ends_open_streams_rather_than_waiting_for_them() {
    let (_forge, _upstream, mut app) = tracking_one().await;
    let _events = app.events().await;
    tokio::time::timeout(Duration::from_secs(10), app.stop())
        .await
        .expect("the app stops with an event stream open");
}
