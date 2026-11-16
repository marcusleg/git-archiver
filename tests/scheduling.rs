//! When fetches run, driven by the test clock and observed through `/metrics`.

mod common;

use common::forge::{FakeForge, Upstream};
use common::{HOUR, TestApp, config_tracking};

/// Three upstream repos with one commit each, and their repo identities.
fn three_repos(forge: &FakeForge) -> (Vec<Upstream>, Vec<String>) {
    let repos: Vec<Upstream> = ["a/one", "a/two", "a/three"]
        .iter()
        .map(|path| {
            let upstream = forge.create_repo(path);
            upstream.commit("first");
            upstream
        })
        .collect();
    let ids = ["a/one", "a/two", "a/three"]
        .iter()
        .map(|path| format!("http://{}/{path}", forge.host()))
        .collect();
    (repos, ids)
}

fn urls(repos: &[Upstream]) -> Vec<&str> {
    repos.iter().map(Upstream::url).collect()
}

/// Each repo's last success, in Unix seconds, in the order given.
async fn last_successes(app: &TestApp, ids: &[String]) -> Vec<Option<u64>> {
    let metrics = app.metrics().await;
    ids.iter().map(|id| metrics.last_success(id)).collect()
}

#[tokio::test]
async fn three_repos_in_a_24h_interval_are_fetched_one_every_8_hours_oldest_first() {
    let forge = FakeForge::start().await;
    let (repos, ids) = three_repos(&forge);
    let app = TestApp::tracking(&urls(&repos)).await;
    let start = app.now();
    let h = |hours: u64| Some(start + hours * 3600);

    app.advance_and_settle(7 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [None, None, None]);

    app.advance_and_settle(HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [h(8), None, None]);

    app.advance_and_settle(8 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [h(8), h(16), None]);

    app.advance_and_settle(8 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [h(8), h(16), h(24)]);

    app.advance_and_settle(8 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [h(32), h(16), h(24)]);

    let next = app
        .metrics()
        .await
        .value("archiver_next_fetch_timestamp_seconds", &[]);
    assert_eq!(next, h(40).map(|t| t as f64));
}

#[tokio::test]
async fn restarting_with_a_fetch_overdue_runs_just_that_fetch_right_away() {
    let forge = FakeForge::start().await;
    let (repos, ids) = three_repos(&forge);
    let mut app = TestApp::tracking(&urls(&repos)).await;
    let start = app.now();
    let h = |hours: u64| Some(start + hours * 3600);
    app.advance_and_settle(8 * HOUR).await;

    app.stop().await;
    app.advance(20 * HOUR);
    app.start_again().await;
    app.settle().await;

    assert_eq!(last_successes(&app, &ids).await, [h(8), h(28), None]);
    let next = app
        .metrics()
        .await
        .value("archiver_next_fetch_timestamp_seconds", &[]);
    assert_eq!(next, h(36).map(|t| t as f64));
}

#[tokio::test]
async fn restarting_with_nothing_overdue_fetches_nothing_until_the_next_due_time() {
    let forge = FakeForge::start().await;
    let (repos, ids) = three_repos(&forge);
    let mut app = TestApp::tracking(&urls(&repos)).await;
    let start = app.now();
    let h = |hours: u64| Some(start + hours * 3600);
    app.advance_and_settle(8 * HOUR).await;

    app.stop().await;
    app.advance(4 * HOUR);
    app.start_again().await;
    app.settle().await;
    assert_eq!(last_successes(&app, &ids).await, [h(8), None, None]);
    let next = app
        .metrics()
        .await
        .value("archiver_next_fetch_timestamp_seconds", &[]);
    assert_eq!(next, h(16).map(|t| t as f64), "due from the saved state");

    app.advance_and_settle(3 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [h(8), None, None]);

    app.advance_and_settle(HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [h(8), h(16), None]);
}

#[tokio::test]
async fn a_repo_added_to_the_config_by_hand_is_fetched_from_the_next_fetch_on_and_the_gap_adjusts()
{
    let forge = FakeForge::start().await;
    let (repos, ids) = three_repos(&forge);
    let app = TestApp::tracking(&urls(&repos[..2])).await;
    let start = app.now();
    let h = |hours: u64| Some(start + hours * 3600);
    app.advance_and_settle(12 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [h(12), None, None]);

    app.write_config(&config_tracking(&urls(&repos)));
    app.advance_and_settle(8 * HOUR).await;

    // Due 24h / 3 after the last attempt; never-attempted repos tie by
    // config order.
    assert_eq!(last_successes(&app, &ids).await, [h(12), h(20), None]);
    let next = app
        .metrics()
        .await
        .value("archiver_next_fetch_timestamp_seconds", &[]);
    assert_eq!(next, h(28).map(|t| t as f64));

    app.advance_and_settle(8 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [h(12), h(20), h(28)]);
}

#[tokio::test]
async fn a_failed_fetch_counts_as_an_attempt_and_the_next_repo_still_gets_its_turn() {
    let forge = FakeForge::start().await;
    let good = forge.create_repo("a/good");
    good.commit("first");
    let missing = format!("http://{}/a/missing.git", forge.host());
    let app = TestApp::tracking(&[&missing, good.url()]).await;
    let start = app.now();
    let h = |hours: u64| Some(start + hours * 3600);
    let ids = [
        format!("http://{}/a/missing", forge.host()),
        format!("http://{}/a/good", forge.host()),
    ];

    app.advance_and_settle(12 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [None, None]);
    let next = app
        .metrics()
        .await
        .value("archiver_next_fetch_timestamp_seconds", &[]);
    assert_eq!(next, h(24).map(|t| t as f64), "the failure was an attempt");

    app.advance_and_settle(12 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [None, h(24)]);
}

#[tokio::test]
async fn a_fetch_that_hangs_is_stopped_after_an_hour_and_counts_as_an_attempt() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("a/stuck");
    upstream.commit("first");
    forge.hang("a/stuck");
    let app = TestApp::tracking(&[upstream.url()]).await;
    let start = app.now();
    let id = format!("http://{}/a/stuck", forge.host());

    app.advance(24 * HOUR);
    forge.wait_for_hung_request().await;
    app.advance_and_settle(HOUR).await;

    let metrics = app.metrics().await;
    assert_eq!(metrics.last_success(&id), None);
    assert_eq!(
        metrics.value("archiver_next_fetch_timestamp_seconds", &[]),
        Some((start + 48 * 3600) as f64),
        "the attempt at 24h counts"
    );
}
