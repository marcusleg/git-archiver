//! The list page in a real browser, at a desktop and a phone viewport.

mod common;

use common::TestApp;
use common::browser::{Browser, Viewport, find_browser_in};
use common::forge::FakeForge;

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

#[test]
fn a_missing_browser_is_an_error_naming_the_variable_to_set() {
    let empty = Some(std::ffi::OsString::from("/nonexistent"));
    let error = find_browser_in(None, empty.clone()).unwrap_err();
    assert!(error.contains("GIT_ARCHIVER_TEST_BROWSER"), "{error}");
    let error = find_browser_in(Some("/no/such/chrome".into()), empty).unwrap_err();
    assert!(error.contains("GIT_ARCHIVER_TEST_BROWSER"), "{error}");
}
