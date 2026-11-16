mod common;

use common::TestApp;
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
