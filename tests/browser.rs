//! The list page in a real browser, at a desktop and a phone viewport.

mod common;

use std::collections::BTreeSet;
use std::time::{Duration, UNIX_EPOCH};

use common::browser::{Browser, Viewport, find_browser_in};
use common::forge::{Failure, FakeForge};
use common::{HOUR, MINUTE, TestApp};

/// Opening the untrack dialog on a repo's page and cancelling it leaves the
/// repo tracked.
async fn cancelling_untrack_keeps_the_repo_tracked(viewport: Viewport) {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("a/one");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    let browser = Browser::launch(viewport).await;
    let tab = browser.open(&format!("http://{}/", app.addr())).await;
    tab.click_row("a/one").await;
    tab.arrives_at(&format!("/repos/{}/a/one", forge.host()))
        .await;

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

/// The add dialog's line, seen only while it's open.
const CLONES_RIGHT_AWAY: &str = "The app clones it right away";

/// Adding a repo through the dialog: a failed add reopens it with the URL
/// and the error, Cancel closes it, and a good URL lands on the list.
async fn adding_a_repo_through_the_dialog(viewport: Viewport) {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("a/one");
    upstream.commit("first");
    let app = TestApp::start().await;
    let browser = Browser::launch(viewport).await;
    let tab = browser.open(&format!("http://{}/", app.addr())).await;

    tab.sees("No repos tracked yet").await;
    tab.doesnt_see(CLONES_RIGHT_AWAY).await;
    tab.click_button("Add repo").await;
    tab.sees(CLONES_RIGHT_AWAY).await;
    tab.sees_a_modal_dialog().await;

    // A URL the browser accepts but the app doesn't.
    let ftp = "ftp://example.com/foo/bar";
    tab.fill("Repo URL", ftp).await;
    tab.click_button("Add repo").await;
    tab.sees("must be an http:// or https:// URL").await;
    assert_eq!(tab.value_of("Repo URL").await, ftp);
    tab.sees_a_modal_dialog().await;

    tab.click_button("Cancel").await;
    tab.stops_seeing(CLONES_RIGHT_AWAY).await;
    tab.doesnt_see("must be an http:// or https:// URL").await;

    tab.click_button("Add repo").await;
    tab.fill("Repo URL", upstream.url()).await;
    tab.click_button("Add repo").await;
    tab.stops_seeing("No repos tracked yet").await;
    tab.sees("a/one").await;
    tab.doesnt_see(CLONES_RIGHT_AWAY).await;
    assert_eq!(tab.path().await, "/");
    assert!(app.config().contains(upstream.url()));

    // Now the button sits beside the "Tracked repos" heading.
    tab.click_button("Add repo").await;
    tab.sees(CLONES_RIGHT_AWAY).await;
    tab.click_button("Cancel").await;
    tab.stops_seeing(CLONES_RIGHT_AWAY).await;
}

#[tokio::test]
async fn adding_a_repo_through_the_dialog_on_desktop() {
    adding_a_repo_through_the_dialog(Viewport::Desktop).await;
}

#[tokio::test]
async fn adding_a_repo_through_the_dialog_on_a_phone() {
    adding_a_repo_through_the_dialog(Viewport::Phone).await;
}

#[tokio::test]
async fn without_javascript_the_add_dialog_opens_and_its_form_submits() {
    let forge = FakeForge::start().await;
    let first = forge.create_repo("a/one");
    first.commit("first");
    let second = forge.create_repo("a/two");
    second.commit("first");
    let app = TestApp::tracking(&[first.url()]).await;
    let browser = Browser::launch(Viewport::Desktop).await;
    let tab = browser
        .open_without_javascript(&format!("http://{}/", app.addr()))
        .await;

    tab.doesnt_see(CLONES_RIGHT_AWAY).await;
    tab.click_button("Add repo").await;
    tab.sees(CLONES_RIGHT_AWAY).await;

    let ftp = "ftp://example.com/foo/bar";
    tab.fill("Repo URL", ftp).await;
    tab.click_dialog_button("Add repo").await;
    tab.sees("must be an http:// or https:// URL").await;
    assert_eq!(tab.value_of("Repo URL").await, ftp);

    tab.fill("Repo URL", second.url()).await;
    tab.click_dialog_button("Add repo").await;
    tab.sees("a/two").await;
    tab.doesnt_see(CLONES_RIGHT_AWAY).await;
    assert_eq!(tab.path().await, "/");
    assert!(app.config().contains(second.url()));
}

/// A forge with two repos, `a/one` and `a/two`, both tracked and fetched.
async fn app_tracking_two_repos() -> (FakeForge, TestApp, [String; 2]) {
    let forge = FakeForge::start().await;
    let urls = ["a/one", "a/two"].map(|path| {
        let upstream = forge.create_repo(path);
        upstream.commit("first");
        upstream.url().to_owned()
    });
    let app = TestApp::tracking(&[&urls[0], &urls[1]]).await;
    app.advance_and_settle(24 * HOUR).await;
    (forge, app, urls)
}

#[tokio::test]
async fn clicking_a_row_on_a_desktop_shows_that_repo_in_the_pane() {
    let (forge, app, [one, two]) = app_tracking_two_repos().await;
    let browser = Browser::launch(Viewport::Desktop).await;
    let tab = browser.open(&format!("http://{}/", app.addr())).await;
    tab.sees("Tracked repos").await;
    tab.sees(&one).await;
    tab.doesnt_see(&two).await;

    tab.click_row("a/two").await;

    let page = format!("/repos/{}/a/two", forge.host());
    tab.arrives_at(&page).await;
    tab.sees(&two).await;
    tab.doesnt_see(&one).await;
    tab.sees("Tracked repos").await;
    tab.doesnt_see("All repos").await;

    tab.go_back().await;
    tab.sees(&one).await;
    tab.doesnt_see(&two).await;
}

#[tokio::test]
async fn on_a_phone_the_list_and_each_repos_card_get_a_page_of_their_own() {
    let (forge, app, [one, two]) = app_tracking_two_repos().await;
    let browser = Browser::launch(Viewport::Phone).await;
    let tab = browser.open(&format!("http://{}/", app.addr())).await;

    // The list only, keeping each repo's name without its host and its
    // status icon.
    tab.sees("Tracked repos").await;
    tab.sees("a/one").await;
    tab.sees("a/two").await;
    for hidden in [
        one.as_str(),
        &forge.host(),
        "Last success",
        "Size",
        "Default branch",
    ] {
        tab.doesnt_see(hidden).await;
    }
    assert_eq!(tab.shown(".repo-table .status svg").await.len(), 2);
    assert_eq!(
        tab.shown(".repo-table .status .label").await,
        Vec::<String>::new(),
        "the status label is left to screen readers"
    );

    // A row opens the card only.
    tab.click_row("a/two").await;
    tab.arrives_at(&format!("/repos/{}/a/two", forge.host()))
        .await;
    tab.sees("All repos").await;
    tab.sees(&two).await;
    tab.sees("Default branch").await;
    tab.doesnt_see("Tracked repos").await;
    tab.doesnt_see("a/one").await;

    // "All repos" goes back to the list.
    tab.click_link("All repos").await;
    tab.arrives_at("/").await;
    tab.sees("Tracked repos").await;
    tab.doesnt_see("Default branch").await;

    // So does the back button.
    tab.click_row("a/one").await;
    tab.arrives_at(&format!("/repos/{}/a/one", forge.host()))
        .await;
    tab.sees(&one).await;
    tab.go_back().await;
    tab.sees("Tracked repos").await;
    tab.doesnt_see(&one).await;
}

#[tokio::test]
async fn without_javascript_the_rows_are_links_that_navigate() {
    let (forge, app, [one, two]) = app_tracking_two_repos().await;
    let browser = Browser::launch(Viewport::Desktop).await;
    let tab = browser
        .open_without_javascript(&format!("http://{}/", app.addr()))
        .await;
    tab.sees(&one).await;

    tab.click_link("a/two").await;

    tab.arrives_at(&format!("/repos/{}/a/two", forge.host()))
        .await;
    tab.sees(&two).await;
    tab.doesnt_see(&one).await;
}

#[test]
fn a_missing_browser_is_an_error_naming_the_variable_to_set() {
    let empty = Some(std::ffi::OsString::from("/nonexistent"));
    let error = find_browser_in(None, empty.clone()).unwrap_err();
    assert!(error.contains("GIT_ARCHIVER_TEST_BROWSER"), "{error}");
    let error = find_browser_in(Some("/no/such/chrome".into()), empty).unwrap_err();
    assert!(error.contains("GIT_ARCHIVER_TEST_BROWSER"), "{error}");
}
