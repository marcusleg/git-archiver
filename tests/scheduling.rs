//! When fetches run, driven by the test clock and observed through `/metrics`.

mod common;

use common::forge::{Failure, FakeForge};
use common::{HOUR, MINUTE, TestApp, config_tracking, last_successes, three_repos, urls};

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

    let next = app.next_fetch().await;
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
    let next = app.next_fetch().await;
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
    let next = app.next_fetch().await;
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
    let next = app.next_fetch().await;
    assert_eq!(next, h(28).map(|t| t as f64));

    app.advance_and_settle(8 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [h(12), h(20), h(28)]);
}

#[tokio::test]
async fn a_repo_untracked_by_hand_widens_the_gap_from_the_next_fetch_on() {
    let forge = FakeForge::start().await;
    let (repos, ids) = three_repos(&forge);
    let app = TestApp::tracking(&urls(&repos)).await;
    let start = app.now();
    let h = |hours: u64| Some(start + hours * 3600);
    app.advance_and_settle(8 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [h(8), None, None]);
    let next = app.next_fetch().await;
    assert_eq!(
        next,
        h(16).map(|t| t as f64),
        "24h / 3 after the last attempt"
    );

    app.write_config(&config_tracking(&urls(&repos[..2])));
    app.advance_and_settle(MINUTE).await;

    let next = app.next_fetch().await;
    assert_eq!(
        next,
        h(20).map(|t| t as f64),
        "24h / 2 after the last attempt"
    );

    app.advance_and_settle(12 * HOUR - MINUTE).await;
    assert_eq!(last_successes(&app, &ids).await, [h(8), h(20), None]);
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
    let next = app.next_fetch().await;
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
        metrics.next_fetch(),
        Some((start + 48 * 3600) as f64),
        "the attempt at 24h counts"
    );
}

#[tokio::test]
async fn a_permanently_failing_repo_among_three_never_holds_up_the_other_two() {
    let forge = FakeForge::start().await;
    let (repos, ids) = three_repos(&forge);
    forge.fail("a/two", Failure::ServerError);
    let app = TestApp::tracking(&urls(&repos)).await;
    let start = app.now();
    let h = |hours: u64| Some(start + hours * 3600);

    for _ in 0..3 {
        app.advance_and_settle(8 * HOUR).await;
    }
    assert_eq!(last_successes(&app, &ids).await, [h(8), None, h(24)]);

    for _ in 0..3 {
        app.advance_and_settle(8 * HOUR).await;
    }
    assert_eq!(last_successes(&app, &ids).await, [h(32), None, h(48)]);
    let metrics = app.metrics().await;
    assert_eq!(metrics.repo_status(&ids[1]), Some("fetch_failed"));
    assert_eq!(metrics.fetch_errors("fetch_failed"), Some(2.0));
}
