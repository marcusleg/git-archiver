//! The list page in a real browser, at a desktop and a phone viewport.

mod common;

use std::collections::BTreeSet;
use std::time::{Duration, UNIX_EPOCH};

use common::browser::{Browser, Viewport, find_browser_in};
use common::forge::{Failure, FakeForge};
use common::{HOUR, MINUTE, TestApp};

/// Opening the untrack dialog and cancelling it leaves the repo tracked.
async fn cancelling_untrack_keeps_the_repo_tracked(viewport: Viewport) {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("a/one");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    let browser = Browser::launch(viewport).await;
    let tab = browser.open(&format!("http://{}/", app.addr())).await;

    tab.sees(upstream.url()).await;
    tab.doesnt_see("Keep archive").await;

    tab.click_button("Untrack").await;
    tab.sees("Keep archive").await;

    tab.click_button("Cancel").await;
    tab.stops_seeing("Keep archive").await;
    tab.sees(upstream.url()).await;

    // Still tracked after a fresh look at the page, too.
    tab.reload().await;
    tab.sees(upstream.url()).await;
    tab.doesnt_see("Untracked archives").await;
    assert!(app.config().contains(upstream.url()));
}

#[tokio::test]
async fn cancelling_untrack_keeps_the_repo_tracked_on_desktop() {
    cancelling_untrack_keeps_the_repo_tracked(Viewport::Desktop).await;
}

#[tokio::test]
async fn cancelling_untrack_keeps_the_repo_tracked_on_a_phone() {
    cancelling_untrack_keeps_the_repo_tracked(Viewport::Phone).await;
}

/// With the page open, "N minutes ago" counts up once a minute in the
/// server's wording, by the page's own clock, while dates stay as they are.
#[tokio::test]
async fn relative_times_count_up_while_the_page_is_open() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("a/one");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    forge.fail("a/one", Failure::DmcaTakedown);
    app.advance_and_settle(24 * HOUR).await;
    let browser = Browser::launch(Viewport::Desktop).await;
    let now = UNIX_EPOCH + Duration::from_secs(app.now());
    let tab = browser
        .open_at(&format!("http://{}/", app.addr()), now)
        .await;

    // Unavailable since a date, last success a day ago, last attempt now.
    let reads = |last_success: &str, last_attempt: &str| -> BTreeSet<String> {
        ["18 Nov 2026", last_success, last_attempt]
            .map(str::to_owned)
            .into()
    };
    assert_eq!(tab.times().await, reads("1 day ago", "just now"));

    let mut passed = Duration::ZERO;
    for (after, last_success, last_attempt) in [
        (MINUTE / 2, "1 day ago", "just now"),
        (MINUTE + MINUTE / 2, "1 day ago", "1 minute ago"),
        (2 * MINUTE + MINUTE / 2, "1 day ago", "2 minutes ago"),
        (59 * MINUTE + MINUTE / 2, "1 day ago", "59 minutes ago"),
        (HOUR + MINUTE / 2, "1 day ago", "1 hour ago"),
        (2 * HOUR + MINUTE / 2, "1 day ago", "2 hours ago"),
        (23 * HOUR + MINUTE / 2, "1 day ago", "23 hours ago"),
        (24 * HOUR + MINUTE / 2, "2 days ago", "1 day ago"),
        (48 * HOUR + MINUTE / 2, "3 days ago", "2 days ago"),
    ] {
        tab.let_time_pass(after - passed).await;
        passed = after;
        assert_eq!(
            tab.times().await,
            reads(last_success, last_attempt),
            "{passed:?} after the page loaded"
        );
    }
}

#[test]
fn a_missing_browser_is_an_error_naming_the_variable_to_set() {
    let empty = Some(std::ffi::OsString::from("/nonexistent"));
    let error = find_browser_in(None, empty.clone()).unwrap_err();
    assert!(error.contains("GIT_ARCHIVER_TEST_BROWSER"), "{error}");
    let error = find_browser_in(Some("/no/such/chrome".into()), empty).unwrap_err();
    assert!(error.contains("GIT_ARCHIVER_TEST_BROWSER"), "{error}");
}
