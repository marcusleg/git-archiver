//! The list page in a real browser, at a desktop and a phone viewport.

mod common;

use std::collections::BTreeSet;
use std::time::{Duration, UNIX_EPOCH};

use common::browser::{Browser, Viewport, find_browser_in};
use common::forge::{Failure, FakeForge};
use common::{HOUR, MINUTE, TestApp, config_tracking};

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

/// Forty tracked repos, enough to scroll: `a/one`, which exists upstream,
/// first, then `b/r00` to `b/r38`, which don't. Forty split the interval
/// into whole seconds. Returns their URLs in the order the list shows them.
async fn app_tracking_a_long_list() -> (FakeForge, TestApp, Vec<String>) {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("a/one");
    upstream.commit("first");
    let mut urls = vec![upstream.url().to_owned()];
    urls.extend((0..39).map(|i| format!("http://{}/b/r{i:02}", forge.host())));
    let repos: Vec<&str> = urls.iter().map(String::as_str).collect();
    let app = TestApp::tracking(&repos).await;
    (forge, app, urls)
}

#[tokio::test]
async fn on_a_desktop_selecting_rows_keeps_the_scroll_and_moves_through_the_history() {
    let (forge, app, urls) = app_tracking_a_long_list().await;
    let browser = Browser::launch(Viewport::Desktop).await;
    let tab = browser.open(&format!("http://{}/", app.addr())).await;
    let identity = |name: &str| format!("{}/{name}", forge.host());
    let page = |name: &str| format!("/repos/{}", identity(name));
    let selected = async |name: &str| {
        let url = urls
            .iter()
            .find(|url| url.trim_end_matches(".git").ends_with(name))
            .expect("a repo in the list");
        tab.shows_selected(&identity(name), url).await;
    };
    selected("a/one").await;
    tab.mark_document().await;

    // A row far down the list: the list stays where it was scrolled.
    let scrolled = tab.scroll_row_into_view("b/r30").await;
    assert!(scrolled > 0.0, "the page scrolls");
    tab.click_row("b/r30").await;
    tab.arrives_at(&page("b/r30")).await;
    selected("b/r30").await;
    assert_eq!(tab.scroll_y().await, scrolled);
    tab.doesnt_see("All repos").await;

    let scrolled = tab.scroll_row_into_view("b/r35").await;
    tab.click_row("b/r35").await;
    tab.arrives_at(&page("b/r35")).await;
    selected("b/r35").await;
    assert_eq!(tab.scroll_y().await, scrolled);
    tab.still_shows_the_marked_document().await;

    // Back and forward move between the selections.
    tab.go_back().await;
    selected("b/r30").await;
    tab.go_back().await;
    tab.arrives_at("/").await;
    selected("a/one").await;
    tab.go_forward().await;
    selected("b/r30").await;
    tab.still_shows_the_marked_document().await;

    // The URL holds the selection.
    tab.reload().await;
    selected("b/r30").await;
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

/// The page while open: a fetch shows in the row and the card without a
/// reload.
async fn a_fetch_updates_the_open_page(viewport: Viewport) {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("a/one");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    let browser = Browser::launch(viewport).await;
    let tab = browser
        .open(&format!(
            "http://{}/repos/{}/a/one",
            app.addr(),
            forge.host()
        ))
        .await;
    tab.sees("Not fetched yet").await;
    tab.mark_document().await;

    app.advance_and_settle(24 * HOUR).await;

    tab.sees("Fetched just now").await;
    tab.stops_seeing("Not fetched yet").await;
    tab.still_shows_the_marked_document().await;
}

#[tokio::test]
async fn a_fetch_updates_the_open_page_on_desktop() {
    a_fetch_updates_the_open_page(Viewport::Desktop).await;
}

#[tokio::test]
async fn a_fetch_updates_the_open_page_on_a_phone() {
    a_fetch_updates_the_open_page(Viewport::Phone).await;
}

#[tokio::test]
async fn a_fetch_updates_the_row_on_the_list_page() {
    let (forge, app, _urls) = app_tracking_two_repos().await;
    let browser = Browser::launch(Viewport::Desktop).await;
    let tab = browser.open(&format!("http://{}/", app.addr())).await;
    tab.mark_document().await;
    tab.doesnt_see("Unavailable").await;

    forge.fail("a/two", Failure::DmcaTakedown);
    app.advance_and_settle(24 * HOUR).await;

    tab.sees("Unavailable · 403").await;
    tab.still_shows_the_marked_document().await;
}

#[tokio::test]
async fn a_dropped_event_stream_shows_a_note_until_it_reconnects_and_the_page_catches_up() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("a/one");
    upstream.commit("first");
    let mut app = TestApp::tracking(&[upstream.url()]).await;
    let browser = Browser::launch(Viewport::Desktop).await;
    let tab = browser.open(&format!("http://{}/", app.addr())).await;
    tab.doesnt_see("Live updates paused").await;
    tab.mark_document().await;

    app.stop().await;
    tab.sees("Live updates paused").await;

    // The page can only learn of what changed while it was away by
    // refetching on reconnect.
    let second = forge.create_repo("a/two");
    app.write_config(&config_tracking(&[upstream.url(), second.url()]));
    app.start_again().await;
    tab.sees("a/two").await;
    tab.stops_seeing("Live updates paused").await;
    tab.still_shows_the_marked_document().await;
}

#[tokio::test]
async fn an_update_waits_while_a_dialog_is_open() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("a/one");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    let browser = Browser::launch(Viewport::Desktop).await;
    let tab = browser.open(&format!("http://{}/", app.addr())).await;
    tab.mark_document().await;
    tab.click_button("Add repo").await;
    let typed = "https://example.com/half/typed";
    tab.fill("Repo URL", typed).await;

    let mut events = app.events().await;
    app.advance_and_settle(24 * HOUR).await;
    events.changed().await;
    // As long again for the page to have heard of it.
    tokio::time::sleep(Duration::from_millis(500)).await;

    tab.sees_a_modal_dialog().await;
    assert_eq!(tab.value_of("Repo URL").await, typed);
    tab.sees("Not fetched yet").await;

    tab.click_button("Cancel").await;
    tab.stops_seeing(CLONES_RIGHT_AWAY).await;
    tab.sees("Fetched just now").await;
    tab.doesnt_see("Not fetched yet").await;
    tab.still_shows_the_marked_document().await;
}

/// What a failed untrack says when the config file is broken.
const UNTRACK_FAILED: &str = "Couldn’t untrack the repo: the config file is invalid";

#[tokio::test]
async fn an_update_keeps_the_selection_the_scroll_position_and_a_failed_action_banner() {
    let (forge, app, urls) = app_tracking_a_long_list().await;
    let browser = Browser::launch(Viewport::Desktop).await;
    let tab = browser.open(&format!("http://{}/", app.addr())).await;
    tab.click_row("b/r30").await;
    tab.arrives_at(&format!("/repos/{}/b/r30", forge.host()))
        .await;

    // Untracking fails on a broken config the app hasn't read yet.
    app.write_config("repos = [");
    tab.click_button("Untrack").await;
    tab.click_button("Keep archive").await;
    tab.sees(UNTRACK_FAILED).await;
    tab.mark_document().await;
    let scrolled = tab.scroll_to(600).await;
    assert!(scrolled > 0.0, "the page scrolls");

    // The app reads the broken config, and the page shows that.
    app.advance_and_settle(MINUTE).await;
    tab.sees("so the app keeps using the last good config")
        .await;

    tab.still_shows_the_marked_document().await;
    tab.sees(UNTRACK_FAILED).await;
    assert_eq!(
        tab.shown(".repo-table [aria-current]").await,
        [format!("{}/b/r30", forge.host())]
    );
    assert_eq!(tab.shown(".pane .repo-url").await, [urls[31].clone()]);
    assert_eq!(tab.scroll_y().await, scrolled);
}

#[tokio::test]
async fn a_repo_page_whose_repo_is_untracked_and_deleted_elsewhere_moves_to_the_list() {
    let (forge, app, [one, two]) = app_tracking_two_repos().await;
    let browser = Browser::launch(Viewport::Desktop).await;
    let page = format!("/repos/{}/a/two", forge.host());
    let tab = browser.open(&format!("http://{}{page}", app.addr())).await;
    tab.sees(&two).await;
    tab.mark_document().await;

    app.untrack_from_page(&page, "Delete archive").await;

    tab.arrives_at("/").await;
    tab.sees(&one).await;
    tab.stops_seeing("a/two").await;
    tab.still_shows_the_marked_document().await;
}

#[test]
fn a_missing_browser_is_an_error_naming_the_variable_to_set() {
    let empty = Some(std::ffi::OsString::from("/nonexistent"));
    let error = find_browser_in(None, empty.clone()).unwrap_err();
    assert!(error.contains("GIT_ARCHIVER_TEST_BROWSER"), "{error}");
    let error = find_browser_in(Some("/no/such/chrome".into()), empty).unwrap_err();
    assert!(error.contains("GIT_ARCHIVER_TEST_BROWSER"), "{error}");
}
