mod common;

use common::forge::{Failure, FakeForge};
use common::git::refs;
use common::{HOUR, TestApp};
use scraper::{Html, Selector};

fn select<'a>(html: &'a Html, selector: &str) -> Vec<scraper::ElementRef<'a>> {
    html.select(&Selector::parse(selector).unwrap()).collect()
}

fn text(element: &scraper::ElementRef) -> String {
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[tokio::test]
async fn the_list_page_shows_one_card_per_tracked_repo() {
    let app = TestApp::with_config(
        r#"repos = [
  "https://github.com/foo/bar",
  "https://gitlab.com/group/sub/project.git",
]"#,
    )
    .await;

    let page = app.get("/").await;
    assert_eq!(page.status, 200);
    assert!(
        page.content_type.starts_with("text/html"),
        "{}",
        page.content_type
    );

    let html = Html::parse_document(&page.text());
    let cards = select(&html, "main article");
    assert_eq!(cards.len(), 2);
    assert!(text(&cards[0]).contains("https://github.com/foo/bar"));
    assert!(text(&cards[1]).contains("https://gitlab.com/group/sub/project.git"));
    assert!(
        select(&html, "title")
            .iter()
            .any(|t| text(t) == "Git Archiver")
    );
    assert!(
        select(&html, "header")
            .iter()
            .any(|h| text(h).contains("Git Archiver"))
    );
}

#[tokio::test]
async fn the_list_page_shows_an_empty_state_when_no_repos_are_tracked() {
    let app = TestApp::start().await;

    let html = Html::parse_document(&app.get("/").await.text());
    assert!(select(&html, "main article").is_empty());
    let main = text(&select(&html, "main")[0]);
    assert!(main.contains("No repos tracked yet"), "{main}");
    assert!(main.contains("config.toml"), "{main}");
}

#[tokio::test]
async fn a_repo_added_to_the_config_shows_after_a_restart() {
    let mut app = TestApp::start().await;
    let config = app.config().replace(
        "repos = [\n",
        "repos = [\n  \"https://github.com/foo/bar\",\n",
    );
    app.write_config(&config);

    app.restart().await;

    let html = Html::parse_document(&app.get("/").await.text());
    let cards = select(&html, "main article");
    assert_eq!(cards.len(), 1);
    assert!(text(&cards[0]).contains("https://github.com/foo/bar"));
}

#[tokio::test]
async fn repo_urls_are_escaped_in_the_page() {
    let app = TestApp::with_config(r#"repos = ["https://example.com/a/<b>"]"#).await;

    let body = app.get("/").await.text();
    assert!(!body.contains("<b>"), "{body}");
}

/// The first element under `element` matching `selector`.
fn first<'a>(element: &scraper::ElementRef<'a>, selector: &str) -> scraper::ElementRef<'a> {
    let selector = Selector::parse(selector).unwrap();
    element.select(&selector).next().unwrap()
}

/// Each `<dt>` in a card with its `<dd>`, in order.
fn field_elements<'a>(
    card: &scraper::ElementRef<'a>,
) -> Vec<(scraper::ElementRef<'a>, scraper::ElementRef<'a>)> {
    let (dt, dd) = (
        Selector::parse("dt").unwrap(),
        Selector::parse("dd").unwrap(),
    );
    card.select(&dt).zip(card.select(&dd)).collect()
}

/// The `<dd>` text for each `<dt>` in a card.
fn fields(card: &scraper::ElementRef) -> Vec<(String, String)> {
    field_elements(card)
        .iter()
        .map(|(name, value)| (text(name), text(value)))
        .collect()
}

/// The `<dd>` for the `<dt>` named `name` in a card.
fn field_value<'a>(card: &scraper::ElementRef<'a>, name: &str) -> scraper::ElementRef<'a> {
    field_elements(card)
        .into_iter()
        .find(|(n, _)| text(n) == name)
        .unwrap_or_else(|| panic!("card has no {name:?} field: {:?}", fields(card)))
        .1
}

/// Checks that a card has no `<dt>` named `name`.
fn assert_no_field(card: &scraper::ElementRef, name: &str) {
    let fields = fields(card);
    assert!(
        fields.iter().all(|(n, _)| n != name),
        "card has a {name:?} field: {fields:?}"
    );
}

fn field(card: &scraper::ElementRef, name: &str) -> String {
    text(&field_value(card, name))
}

/// The `datetime` of the `<time>` in a card's field, if any.
fn field_time(card: &scraper::ElementRef, name: &str) -> Option<String> {
    let time = Selector::parse("time").unwrap();
    let value = field_value(card, name);
    let time = value.select(&time).next()?;
    time.value().attr("datetime").map(str::to_owned)
}

#[tokio::test]
async fn a_fetched_repo_shows_its_status_last_attempt_default_branch_tip_and_size() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo_with_default_branch("foo/bar", "trunk");
    upstream.commit("first");
    let tip = upstream.commit("second");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    app.advance_and_settle(3 * HOUR).await;

    let html = Html::parse_document(&app.get("/").await.text());
    let card = &select(&html, "main article")[0];

    let status = &select(&html, "main article .status")[0];
    assert_eq!(text(status), "Fetched 3 hours ago");
    assert!(
        status
            .select(&Selector::parse("svg.icon-circle-check").unwrap())
            .next()
            .is_some(),
        "{}",
        status.html()
    );
    assert_eq!(field(card, "Last attempt"), "3 hours ago");
    assert_eq!(field(card, "Default branch"), "trunk");

    let sha = &select(&html, "main article dd code.sha")[0];
    assert!(
        text(sha).len() >= 7 && tip.starts_with(&text(sha)),
        "{}",
        text(sha)
    );
    assert_eq!(sha.value().attr("title"), Some(tip.as_str()));

    let size = &select(&html, "main article dd.num")[0];
    let size_text = text(size);
    let (number, unit) = size_text.split_once(' ').unwrap();
    assert!(number.parse::<f64>().unwrap() > 0.0, "{size_text}");
    assert!(["B", "kB", "MB", "GB"].contains(&unit), "{size_text}");

    let css = app.get("/assets/style.css").await.text();
    assert!(css.contains("font-variant-numeric: tabular-nums;"));
    assert!(css.contains("color: var(--accent-green);"));
}

#[tokio::test]
async fn an_upstream_with_no_commits_shows_as_available_with_no_tip() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/empty");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;

    let html = Html::parse_document(&app.get("/").await.text());
    let card = &select(&html, "main article")[0];
    assert_eq!(
        text(&select(&html, "main article .status")[0]),
        "Fetched just now"
    );
    assert_eq!(field(card, "Tip"), "No commits yet");
    assert!(select(&html, "main article code.sha").is_empty());
}

#[tokio::test]
async fn a_repo_not_fetched_yet_says_so() {
    let app = TestApp::tracking(&["https://github.com/foo/bar"]).await;

    let html = Html::parse_document(&app.get("/").await.text());
    let card = &select(&html, "main article")[0];
    assert_eq!(
        text(&select(&html, "main article .status")[0]),
        "Not fetched yet"
    );
    assert_eq!(field(card, "Last attempt"), "Never");
}

#[tokio::test]
async fn a_card_shows_its_preserved_ref_count_and_expands_to_their_names_and_shas() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let old_tip = upstream.commit("first");
    upstream.tag("v1");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    let old_tag = refs(&archive)["refs/tags/v1"].clone();

    let html = Html::parse_document(&app.get("/").await.text());
    let preserved = &select(&html, "main article .preserved")[0];
    assert_eq!(text(preserved), "0 preserved refs");
    assert!(select(&html, "main article details").is_empty());

    upstream.force_push_new_history("rewritten");
    upstream.move_tag("v1");
    app.advance_and_settle(24 * HOUR).await;

    let html = Html::parse_document(&app.get("/").await.text());
    let summary = &select(&html, "main article details.preserved summary")[0];
    assert_eq!(text(summary), "2 preserved refs");
    let items = select(&html, "main article details.preserved li");
    let names: Vec<String> = items
        .iter()
        .map(|item| text(&first(item, "code.ref")))
        .collect();
    assert_eq!(
        names,
        [
            "refs/preserved/20261118T120000Z/heads/main",
            "refs/preserved/20261118T120000Z/tags/v1",
        ]
    );
    for (item, full) in items.iter().zip([&old_tip, &old_tag]) {
        let sha = first(item, "code.sha");
        assert_eq!(sha.value().attr("title"), Some(full.as_str()));
        assert_eq!(text(&sha), full[..12]);
    }
}

#[tokio::test]
async fn a_repo_not_fetched_yet_shows_no_preserved_refs_row() {
    let app = TestApp::tracking(&["https://github.com/foo/bar"]).await;

    let html = Html::parse_document(&app.get("/").await.text());
    assert!(select(&html, "main article .preserved").is_empty());
}

/// Checks that a repo archived 24 hours after start (2026-11-17 12:00
/// UTC), whose next fetch 24 hours later fails with `failure`, shows that
/// last success 3 hours after the failure, under a status line starting
/// with `status`.
async fn assert_shows_last_success_after(failure: Failure, status: &str) {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    forge.fail("foo/bar", failure);
    app.advance_and_settle(24 * HOUR).await;
    app.advance(3 * HOUR);

    let html = Html::parse_document(&app.get("/").await.text());
    let card = &select(&html, "main article")[0];
    let line = text(&select(&html, "main article .status")[0]);
    assert!(line.starts_with(status), "{line}");
    assert_eq!(field(card, "Last success"), "1 day ago");
    assert_eq!(
        field_time(card, "Last success").as_deref(),
        Some("2026-11-17T12:00:00Z")
    );
    assert_eq!(field(card, "Last attempt"), "3 hours ago");
}

#[tokio::test]
async fn an_unavailable_repo_shows_its_last_successful_fetch() {
    assert_shows_last_success_after(Failure::DmcaTakedown, "Unavailable").await;
}

#[tokio::test]
async fn a_repo_whose_fetch_failed_shows_its_last_successful_fetch() {
    assert_shows_last_success_after(Failure::ServerError, "Fetch failed").await;
}

/// Checks that a repo whose first fetch failed with `failure` shows "Never"
/// for its last success, under a status line starting with `status`.
async fn assert_never_fetched_successfully(failure: Failure, status: &str) {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    forge.fail("foo/bar", failure);
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;

    let html = Html::parse_document(&app.get("/").await.text());
    let card = &select(&html, "main article")[0];
    let line = text(&select(&html, "main article .status")[0]);
    assert!(line.starts_with(status), "{line}");
    assert_eq!(field(card, "Last attempt"), "just now");
    assert_eq!(field(card, "Last success"), "Never");
    assert_eq!(field_time(card, "Last success"), None);
}

#[tokio::test]
async fn an_unavailable_repo_never_fetched_successfully_shows_never_for_its_last_success() {
    assert_never_fetched_successfully(Failure::DmcaTakedown, "Unavailable").await;
}

#[tokio::test]
async fn a_repo_whose_first_fetch_failed_shows_never_for_its_last_success() {
    assert_never_fetched_successfully(Failure::ServerError, "Fetch failed").await;
}

#[tokio::test]
async fn an_available_repo_shows_its_last_successful_fetch_only_in_its_status_line() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    app.advance(3 * HOUR);

    let html = Html::parse_document(&app.get("/").await.text());
    let card = &select(&html, "main article")[0];
    assert_eq!(
        text(&select(&html, "main article .status")[0]),
        "Fetched 3 hours ago"
    );
    assert_no_field(card, "Last success");
}

#[tokio::test]
async fn a_repo_not_fetched_yet_shows_no_last_success_field() {
    let app = TestApp::tracking(&["https://github.com/foo/bar"]).await;

    let html = Html::parse_document(&app.get("/").await.text());
    let card = &select(&html, "main article")[0];
    assert_no_field(card, "Last success");
}
