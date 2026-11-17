mod common;

use common::forge::{Failure, FakeForge};
use common::git::refs;
use common::list_page::{ListPage, select, text};
use common::{HOUR, MINUTE, TestApp};
use scraper::{Html, Selector};

#[tokio::test]
async fn the_list_page_shows_one_row_per_tracked_repo_and_the_first_ones_card() {
    let app = TestApp::with_config(
        r#"repos = [
  "https://gitlab.com/group/sub/project.git",
  "https://github.com/foo/bar",
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

    let list = ListPage::parse(&page.text());
    assert_eq!(
        list.columns("Tracked repos"),
        ["Repo", "Status", "Last success", "Size"]
    );
    let rows = list.tracked();
    let repos: Vec<_> = rows
        .iter()
        .map(|r| {
            (
                r.host.as_str(),
                r.path.as_str(),
                r.repo.as_str(),
                r.title.as_str(),
            )
        })
        .collect();
    assert_eq!(
        repos,
        [
            (
                "github.com/",
                "foo/bar",
                "github.com/foo/bar",
                "https://github.com/foo/bar"
            ),
            (
                "gitlab.com/",
                "group/sub/project",
                "gitlab.com/group/sub/project",
                "https://gitlab.com/group/sub/project.git"
            ),
        ]
    );
    assert_eq!(
        rows.iter().map(|r| r.selected).collect::<Vec<_>>(),
        [true, false],
        "the pane's repo is marked"
    );
    assert_eq!(
        list.pane_title().as_deref(),
        Some("https://github.com/foo/bar")
    );
    assert_eq!(select(&list.html, "main article").len(), 1, "one card");
    assert!(
        select(&list.html, "title")
            .iter()
            .any(|t| text(t) == "Git Archiver")
    );
    assert!(
        select(&list.html, "header")
            .iter()
            .any(|h| text(h).contains("Git Archiver"))
    );
}

#[tokio::test]
async fn each_status_shows_its_icon_and_label_with_the_last_success_and_size() {
    let forge = FakeForge::start().await;
    let [ok, legal, broken] = ["a/ok", "a/legal", "a/broken"].map(|path| {
        let upstream = forge.create_repo(path);
        upstream.commit("first");
        upstream
    });
    forge.fail("a/legal", Failure::UnavailableForLegalReasons);
    forge.fail("a/broken", Failure::ServerError);
    let never = "https://github.com/z/never";
    // Four repos in a 24h interval are fetched 6h apart, in config order.
    let app = TestApp::tracking(&[ok.url(), legal.url(), broken.url(), never]).await;
    app.advance_and_settle(6 * HOUR).await;
    app.advance_and_settle(6 * HOUR).await;
    app.advance_and_settle(6 * HOUR).await;

    let rows = app.list_page().await.tracked();
    let cells: Vec<_> = rows
        .iter()
        .map(|r| {
            (
                r.path.as_str(),
                r.status.as_str(),
                r.icon.as_str(),
                r.last_success.as_str(),
            )
        })
        .collect();
    assert_eq!(
        cells,
        [
            (
                "a/broken",
                "Fetch failed",
                "icon icon-refresh-cw-off",
                "Never"
            ),
            ("a/legal", "Unavailable · 451", "icon icon-ban", "Never"),
            (
                "a/ok",
                "Available",
                "icon icon-circle-check",
                "12 hours ago"
            ),
            ("z/never", "Not fetched yet", "icon icon-clock", "Never"),
        ]
    );
    assert_eq!(
        rows[2].last_success_time.as_deref(),
        Some("2026-11-16T18:00:00Z")
    );
    assert_eq!(rows[0].last_success_time, None);

    let (number, unit) = rows[2].size.split_once(' ').unwrap();
    assert!(number.parse::<f64>().unwrap() > 0.0, "{}", rows[2].size);
    assert!(["B", "kB", "MB", "GB"].contains(&unit), "{}", rows[2].size);
    assert_eq!(rows[3].size, "—");
}

#[tokio::test]
async fn the_list_page_shows_an_empty_state_and_no_pane_when_no_repos_are_tracked() {
    let app = TestApp::start().await;

    let list = app.list_page().await;
    assert!(select(&list.html, "main aside").is_empty());
    assert!(select(&list.html, "main article").is_empty());
    assert!(list.tracked().is_empty());
    let main = text(&select(&list.html, "main")[0]);
    assert!(main.contains("No repos tracked yet"), "{main}");
    assert!(main.contains("config.toml"), "{main}");
}

#[tokio::test]
async fn untracked_archives_show_in_their_table_when_no_repos_are_tracked() {
    let app = TestApp::start().await;
    std::fs::create_dir_all(app.archive("example.com/foo/bar")).unwrap();

    let list = app.list_page().await;
    assert!(select(&list.html, "main aside").is_empty(), "no pane");
    assert!(text(&select(&list.html, "main")[0]).contains("No repos tracked yet"));
    assert_eq!(
        list.columns("Untracked archives"),
        ["Repo", "Status", "Last success", "Size"]
    );
    let rows = list.untracked();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].repo, "example.com/foo/bar");
    assert_eq!(rows[0].title, "https://example.com/foo/bar");
    assert!(!rows[0].selected);
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

    let list = app.list_page().await;
    let titles: Vec<_> = list.tracked().into_iter().map(|r| r.title).collect();
    assert_eq!(titles, ["https://github.com/foo/bar"]);
    assert_eq!(
        list.pane_title().as_deref(),
        Some("https://github.com/foo/bar")
    );
}

#[tokio::test]
async fn repo_urls_are_escaped_in_the_row_its_title_and_the_card() {
    let url = "https://example.com/a/<b>";
    let app = TestApp::with_config(&format!("repos = [{url:?}]")).await;

    let body = app.get("/").await.text();
    assert!(!body.contains("<b>"), "{body}");
    let list = ListPage::parse(&body);
    let row = &list.tracked()[0];
    assert_eq!(row.repo, "example.com/a/%3Cb%3E", "the identity encodes it");
    assert_eq!(row.title, url);
    assert_eq!(list.pane_title().as_deref(), Some(url));
}

#[tokio::test]
async fn banners_span_the_page_above_both_columns() {
    let app = TestApp::tracking(&["https://github.com/foo/bar"]).await;
    app.write_config("repos = [\n");
    app.advance_and_settle(MINUTE).await;

    let list = app.list_page().await;
    let banner = select(&list.html, "main > .banner");
    assert_eq!(
        banner.len(),
        1,
        "the config error banner is a child of main"
    );
    assert!(text(&banner[0]).contains("config.toml"));
    assert!(select(&list.html, "main section .banner, main aside .banner").is_empty());
    assert_eq!(
        select(&list.html, "main > .banner + .columns").len(),
        1,
        "the columns follow the banner"
    );
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

/// The hover titles of the rows in the table of the section headed
/// `heading`, in page order.
fn section_repos(html: &str, heading: &str) -> Vec<String> {
    ListPage::parse(html)
        .rows(heading)
        .into_iter()
        .map(|r| r.title)
        .collect()
}

#[tokio::test]
async fn tracked_repos_show_sorted_by_identity_whatever_the_config_order() {
    let urls = [
        "https://gitlab.com/group/project",
        "https://github.com/foo/zed",
        "https://github.com/foo/bar",
    ];
    let app = TestApp::tracking(&urls).await;

    let html = app.get("/").await.text();
    assert_eq!(
        section_repos(&html, "Tracked repos"),
        [urls[2], urls[1], urls[0]]
    );
}

#[tokio::test]
async fn identities_sort_case_insensitively_and_the_exact_string_breaks_ties() {
    let urls = [
        "https://github.com/zed/repo",
        "https://github.com/NixOS/nixpkgs",
        "https://github.com/foo/bar",
        "https://github.com/nixos/Nixpkgs",
        "https://github.com/Foo/bar",
    ];
    let app = TestApp::tracking(&urls).await;

    let html = app.get("/").await.text();
    assert_eq!(
        section_repos(&html, "Tracked repos"),
        [urls[4], urls[2], urls[1], urls[3], urls[0]]
    );
}

#[tokio::test]
async fn the_scheme_plays_no_part_in_the_order() {
    let urls = [
        "https://example.com/c",
        "http://example.com/b",
        "https://example.com/a",
        "http://example.com/d",
    ];
    let app = TestApp::tracking(&urls).await;

    let html = app.get("/").await.text();
    assert_eq!(
        section_repos(&html, "Tracked repos"),
        [urls[2], urls[1], urls[0], urls[3]]
    );
}

#[tokio::test]
async fn untracked_archives_show_sorted_the_same_way() {
    let app = TestApp::start().await;
    for archive in [
        "example.com/Zed",
        "example.com/nixos/nixpkgs",
        "example.com/Alpha",
    ] {
        std::fs::create_dir_all(app.archive(archive)).unwrap();
    }

    let html = app.get("/").await.text();
    assert_eq!(
        section_repos(&html, "Untracked archives"),
        [
            "https://example.com/Alpha",
            "https://example.com/nixos/nixpkgs",
            "https://example.com/Zed",
        ]
    );
}

#[tokio::test]
async fn sorting_the_page_leaves_the_config_file_unchanged() {
    let config = r#"repos = [
  "https://github.com/zed/repo",
  "https://github.com/foo/bar",
]
"#;
    let app = TestApp::with_config(config).await;

    app.get("/").await;
    assert_eq!(app.config(), config);
}
