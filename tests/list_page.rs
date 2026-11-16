mod common;

use common::forge::FakeForge;
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

/// The `<dd>` text for each `<dt>` in a card.
fn fields(card: &scraper::ElementRef) -> Vec<(String, String)> {
    let (dt, dd) = (
        Selector::parse("dt").unwrap(),
        Selector::parse("dd").unwrap(),
    );
    let names = card.select(&dt).map(|e| text(&e));
    let values = card.select(&dd).map(|e| text(&e));
    names.zip(values).collect()
}

fn field(card: &scraper::ElementRef, name: &str) -> String {
    fields(card)
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("card has no {name:?} field: {:?}", fields(card)))
        .1
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
