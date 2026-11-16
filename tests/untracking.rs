//! Untracking repos from the UI or by hand, and the untracked archives they
//! leave behind.

mod common;

use common::forge::{FakeForge, Upstream};
use common::git::{preserved_refs, refs};
use std::time::Duration;

use common::{HOUR, MINUTE, Page, TestApp, config_tracking, last_successes, three_repos, urls};
use scraper::{ElementRef, Html, Selector};

fn select<'a>(html: &'a Html, selector: &str) -> Vec<ElementRef<'a>> {
    html.select(&Selector::parse(selector).unwrap()).collect()
}

fn select_in<'a>(element: &ElementRef<'a>, selector: &str) -> Vec<ElementRef<'a>> {
    element
        .select(&Selector::parse(selector).unwrap())
        .collect()
}

fn text(element: &ElementRef) -> String {
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The repo names on the cards of the list page section headed `heading`,
/// e.g. "Tracked repos", in page order. Empty if there is no such section.
async fn section(app: &TestApp, heading: &str) -> Vec<String> {
    let html = Html::parse_document(&app.get("/").await.text());
    for section in select(&html, "main section") {
        let Some(h2) = select_in(&section, "h2").into_iter().next() else {
            continue;
        };
        if text(&h2).starts_with(heading) {
            return select_in(&section, "article .repo-url")
                .iter()
                .map(text)
                .collect();
        }
    }
    Vec::new()
}

async fn tracked(app: &TestApp) -> Vec<String> {
    section(app, "Tracked repos").await
}

async fn untracked(app: &TestApp) -> Vec<String> {
    section(app, "Untracked archives").await
}

/// The card on the list page whose repo name is `name`, as HTML.
async fn card(app: &TestApp, name: &str) -> String {
    let html = Html::parse_document(&app.get("/").await.text());
    select(&html, "main article")
        .into_iter()
        .find(|card| select_in(card, ".repo-url").first().map(text).as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no card for {name}"))
        .html()
}

/// Opens the untrack dialog on the card for `url` and clicks the button
/// labelled `choice`, the way a browser submits it.
async fn untrack(app: &TestApp, url: &str, choice: &str) -> Page {
    let html = Html::parse_fragment(&card(app, url).await);
    let form = select(&html, "dialog form")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("the card for {url} has no untrack dialog"));
    assert_eq!(form.value().attr("method"), Some("post"));
    let action = form.value().attr("action").unwrap().to_owned();
    let mut fields: Vec<(String, String)> = select_in(&form, "input[type=hidden]")
        .iter()
        .map(|input| {
            let value = input.value();
            (
                value.attr("name").unwrap().to_owned(),
                value.attr("value").unwrap_or("").to_owned(),
            )
        })
        .collect();
    let button = select_in(&form, "button")
        .into_iter()
        .find(|button| text(button) == choice)
        .unwrap_or_else(|| panic!("the untrack dialog has no {choice:?} button"));
    if let Some(name) = button.value().attr("name") {
        fields.push((
            name.to_owned(),
            button.value().attr("value").unwrap_or("").to_owned(),
        ));
    }
    let fields: Vec<(&str, &str)> = fields
        .iter()
        .map(|(n, v)| (n.as_str(), v.as_str()))
        .collect();
    app.post_form(&action, &fields).await
}

/// A forge with one repo holding a commit.
async fn forge_with(path: &str) -> (FakeForge, Upstream) {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo(path);
    upstream.commit("first");
    (forge, upstream)
}

async fn last_attempt_of(app: &TestApp, name: &str) -> String {
    let html = Html::parse_fragment(&card(app, name).await);
    let names = select(&html, "dt");
    let values = select(&html, "dd");
    names
        .iter()
        .zip(&values)
        .find(|(dt, _)| text(dt) == "Last attempt")
        .map(|(_, dd)| text(dd))
        .unwrap()
}

#[tokio::test]
async fn untracking_with_keep_archive_stops_fetching_and_lists_the_archive_as_untracked() {
    let (forge, upstream) = forge_with("foo/bar").await;
    let other = "https://example.com/other/repo";
    let app = TestApp::tracking(&[upstream.url(), other]).await;
    app.advance_and_settle(12 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    let archived = refs(&archive);
    assert!(archived.contains_key("refs/heads/main"));

    let page = untrack(&app, upstream.url(), "Keep archive").await;
    assert_eq!(page.status, 200, "{}", page.text());

    assert_eq!(app.config(), config_tracking(&[other]));
    assert_eq!(tracked(&app).await, [other]);
    let id = format!("http://{}/foo/bar", forge.host());
    assert_eq!(untracked(&app).await, [id.as_str()]);
    assert_eq!(refs(&archive), archived, "the archive is intact");

    upstream.commit("second");
    app.advance_and_settle(48 * HOUR).await;
    assert_eq!(refs(&archive), archived, "no longer fetched");
    assert_eq!(last_attempt_of(&app, &id).await, "2 days ago");
}

#[tokio::test]
async fn untracking_a_repo_widens_the_gap_from_the_next_fetch_on() {
    let forge = FakeForge::start().await;
    let (repos, ids) = three_repos(&forge);
    let app = TestApp::tracking(&urls(&repos)).await;
    let start = app.now();
    let h = |hours: u64| Some(start + hours * 3600);
    app.advance_and_settle(8 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [h(8), None, None]);
    assert_eq!(
        app.next_fetch().await,
        h(16),
        "24h / 3 after the last attempt"
    );

    let page = untrack(&app, repos[2].url(), "Keep archive").await;
    assert_eq!(page.status, 200, "{}", page.text());
    app.advance_and_settle(MINUTE).await;

    assert_eq!(
        app.next_fetch().await,
        h(20),
        "24h / 2 after the last attempt"
    );
    app.advance_and_settle(12 * HOUR - MINUTE).await;
    assert_eq!(last_successes(&app, &ids).await, [h(8), h(20), None]);
}

#[tokio::test]
async fn untracking_with_delete_archive_erases_the_archive_and_its_state() {
    let (forge, upstream) = forge_with("foo/bar").await;
    let other = "https://example.com/other/repo";
    let app = TestApp::tracking(&[upstream.url(), other]).await;
    app.advance_and_settle(12 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    let id = format!("http://{}/foo/bar", forge.host());
    assert!(archive.exists());
    assert!(app.metrics().await.last_success(&id).is_some());

    let page = untrack(&app, upstream.url(), "Delete archive").await;
    assert_eq!(page.status, 200, "{}", page.text());

    assert_eq!(app.config(), config_tracking(&[other]));
    assert_eq!(tracked(&app).await, [other]);
    assert_eq!(untracked(&app).await, Vec::<String>::new());
    assert!(!archive.exists());
    assert!(
        !archive.parent().unwrap().exists(),
        "no empty directories left behind"
    );
    let state = std::fs::read_to_string(app.data_dir().join("state.json")).unwrap();
    assert!(!state.contains(&id), "{state}");
    let metrics = app.metrics().await;
    assert!(!metrics.text.contains(&id), "{}", metrics.text);

    app.advance_and_settle(48 * HOUR).await;
    assert!(!archive.exists(), "no longer fetched");
}

#[tokio::test]
async fn removing_a_repo_from_the_config_by_hand_keeps_its_archive_as_untracked() {
    let (forge, upstream) = forge_with("foo/bar").await;
    let other = "https://example.com/other/repo";
    let app = TestApp::tracking(&[upstream.url(), other]).await;
    app.advance_and_settle(12 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    let archived = refs(&archive);

    app.write_config(&config_tracking(&[other]));
    app.advance_and_settle(MINUTE).await;

    assert_eq!(tracked(&app).await, [other]);
    let id = format!("http://{}/foo/bar", forge.host());
    assert_eq!(untracked(&app).await, [id]);
    upstream.commit("second");
    app.advance_and_settle(48 * HOUR).await;
    assert_eq!(refs(&archive), archived, "kept, and no longer fetched");
}

/// Tracks `upstream`, fetches it, has upstream rewrite its history so the
/// next fetch preserves the old tip, then untracks it keeping the archive.
/// Returns the app, the archive's path and the preserved ref's name.
async fn untracked_archive_with_preserved_ref(
    forge: &FakeForge,
    upstream: &Upstream,
) -> (TestApp, std::path::PathBuf, String) {
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    upstream.force_push_new_history("rewritten");
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    let preserved: Vec<String> = preserved_refs(&archive).into_keys().collect();
    assert_eq!(preserved.len(), 1, "{preserved:?}");
    let page = untrack(&app, upstream.url(), "Keep archive").await;
    assert_eq!(page.status, 200, "{}", page.text());
    (app, archive, preserved[0].clone())
}

#[tokio::test]
async fn adding_an_untracked_repo_again_in_the_ui_continues_its_archive() {
    let (forge, upstream) = forge_with("foo/bar").await;
    let (app, archive, preserved) = untracked_archive_with_preserved_ref(&forge, &upstream).await;
    let before = refs(&archive)[&preserved].clone();
    let tip = upstream.commit("second");
    let id = format!("http://{}/foo/bar", forge.host());
    let metrics = app.metrics().await;
    assert_eq!(
        metrics.value("archiver_repo_preserved_refs", &[("repo", &id)]),
        Some(1.0),
        "an untracked archive keeps its metrics"
    );

    let page = app.add_repo(upstream.url()).await;
    assert_eq!(page.status, 200, "{}", page.text());
    app.settle().await;

    assert_eq!(tracked(&app).await, [upstream.url()]);
    assert_eq!(untracked(&app).await, Vec::<String>::new());
    let after = refs(&archive);
    assert_eq!(after["refs/heads/main"], tip);
    assert_eq!(after[&preserved], before);
}

#[tokio::test]
async fn adding_an_untracked_repo_again_in_the_config_continues_its_archive() {
    let (forge, upstream) = forge_with("foo/bar").await;
    let (app, archive, preserved) = untracked_archive_with_preserved_ref(&forge, &upstream).await;
    let before = refs(&archive)[&preserved].clone();
    let tip = upstream.commit("second");

    app.write_config(&config_tracking(&[upstream.url()]));
    app.advance_and_settle(24 * HOUR).await;

    assert_eq!(tracked(&app).await, [upstream.url()]);
    assert_eq!(untracked(&app).await, Vec::<String>::new());
    let after = refs(&archive);
    assert_eq!(after["refs/heads/main"], tip);
    assert_eq!(after[&preserved], before);
}

/// The warning banner about the config file, if the list page shows one.
async fn config_banner(app: &TestApp) -> Option<String> {
    let html = Html::parse_document(&app.get("/").await.text());
    select(&html, "main .banner")
        .iter()
        .map(text)
        .find(|banner| banner.contains("config.toml"))
}

#[tokio::test]
async fn an_invalid_config_keeps_the_last_good_repo_list_and_shows_a_warning_until_fixed() {
    let logs = common::logs::capture();
    let forge = FakeForge::start().await;
    let one = forge.create_repo("a/one");
    one.commit("first");
    let two = forge.create_repo("a/two");
    let tip = two.commit("first");
    let app = TestApp::tracking(&[one.url(), two.url()]).await;
    assert_eq!(config_banner(&app).await, None);

    let good = app.config();
    app.write_config(&format!("{good}\ninterval = \"often\"\n"));
    app.advance_and_settle(12 * HOUR).await;
    app.advance_and_settle(12 * HOUR).await;

    let banner = config_banner(&app).await.expect("a warning banner");
    assert!(banner.contains("is invalid"), "{banner}");
    assert!(banner.contains("last good config"), "{banner}");
    assert!(banner.contains("interval"), "names the problem: {banner}");
    assert!(
        !logs.matching("keeping the last good config").is_empty(),
        "{:?}",
        logs.lines()
    );
    let archive = app.archive(&format!("{}/a/two", forge.host()));
    assert_eq!(refs(&archive)["refs/heads/main"], tip, "still fetching");

    app.write_config(&good);
    app.advance_and_settle(MINUTE).await;
    assert_eq!(config_banner(&app).await, None);
    assert_eq!(tracked(&app).await, [one.url(), two.url()]);
}

#[tokio::test]
async fn the_config_warning_never_quotes_the_file() {
    let app = TestApp::tracking(&["https://example.com/foo/bar"]).await;

    app.write_config(
        "repos = [\"https://example.com/foo/bar\"]\n\n[hosts.\"github.com\"]\ntoken = \"ghp_secret\n",
    );
    app.advance_and_settle(MINUTE).await;

    let banner = config_banner(&app).await.expect("a warning banner");
    assert!(banner.contains("line 4"), "says where: {banner}");
    assert!(!banner.contains("ghp_secret"), "{banner}");
}

const GOOD_REPO: &str = "https://example.com/good/repo";

/// What a config warning about credentials shows, for the checks one test
/// makes on its own.
struct CredentialsWarning {
    banner: String,
    page: String,
    logged: String,
}

/// Tracks [`GOOD_REPO`], then writes `bad_config`, which holds the password
/// `secretpw`. Checks what every such warning shares: a banner shows, the
/// page and the log never show `secretpw`, and the last good repo list stays.
async fn write_config_with_credentials(bad_config: &str) -> CredentialsWarning {
    let logs = common::logs::capture();
    let app = TestApp::tracking(&[GOOD_REPO]).await;

    app.write_config(bad_config);
    app.advance_and_settle(MINUTE).await;

    let banner = config_banner(&app).await.expect("a warning banner");
    let page = app.get("/").await.text();
    assert!(!page.contains("secretpw"), "{page}");
    let logged = logs.lines().join("\n");
    assert!(!logged.contains("secretpw"), "{logged}");
    assert_eq!(
        tracked(&app).await,
        [GOOD_REPO],
        "keeps the last good repo list"
    );
    CredentialsWarning {
        banner,
        page,
        logged,
    }
}

#[tokio::test]
async fn the_config_warning_names_a_hosts_key_with_credentials_without_showing_them() {
    let CredentialsWarning {
        banner,
        page,
        logged,
    } = write_config_with_credentials(&format!(
        "{}\n[hosts.\"https://user:secretpw@github.com\"]\ntoken = \"x\"\n",
        config_tracking(&[GOOD_REPO, "https://example.com/new/repo"]),
    ))
    .await;

    assert!(banner.contains("`hosts` key"), "which setting: {banner}");
    assert!(banner.contains("github.com"), "which key: {banner}");
    assert!(banner.contains("must be a host"), "why: {banner}");
    assert!(!page.contains("user:"), "{page}");
    assert!(logged.contains("must be a host"), "{logged}");
    assert!(!logged.contains("user:"), "{logged}");
}

#[tokio::test]
async fn the_config_warning_names_an_unknown_field_with_credentials_without_showing_them() {
    let tables = [
        (
            "[hosts.\"github.com\"]\ntoken = \"a\"\n\"https://user:secretpw@github.com\" = \"x\"\n",
            "expected `token`",
        ),
        (
            "[\"https://user:secretpw@github.com\"]\ntoken = \"x\"\n",
            "expected one of `interval`",
        ),
    ];
    for (table, expected) in tables {
        let CredentialsWarning {
            banner,
            page,
            logged,
        } = write_config_with_credentials(&format!(
            "{}\n{table}",
            config_tracking(&[GOOD_REPO, "https://example.com/new/repo"]),
        ))
        .await;

        assert!(banner.contains("line "), "where: {banner}");
        assert!(banner.contains("column "), "where: {banner}");
        assert!(
            banner.contains("unknown field `https://…@github.com`"),
            "what: {banner}"
        );
        assert!(banner.contains(expected), "what was expected: {banner}");
        assert!(!page.contains("user:"), "{page}");
        assert!(logged.contains("unknown field"), "{logged}");
        assert!(!logged.contains("user:"), "{logged}");
    }
}

#[tokio::test]
async fn the_config_warning_names_a_key_with_an_escaped_at_without_showing_its_credentials() {
    let tables = [
        (
            "[hosts]\n\"https://user:secretpw\\u0040github.com\" = \"ghp_x\"\n",
            "invalid type",
        ),
        (
            "[hosts.\"github.com\"]\ntoken = \"a\"\n\"https://user:secretpw\\u0040github.com\" = \"x\"\n",
            "unknown field `https://…@github.com`, expected `token`",
        ),
    ];
    for (table, expected) in tables {
        let CredentialsWarning {
            banner,
            page,
            logged,
        } = write_config_with_credentials(&format!(
            "{}\n{table}",
            config_tracking(&[GOOD_REPO, "https://example.com/new/repo"]),
        ))
        .await;

        assert!(banner.contains("line "), "where: {banner}");
        assert!(banner.contains("column "), "where: {banner}");
        assert!(banner.contains(expected), "what: {banner}");
        assert!(!page.contains("user:"), "{page}");
        assert!(logged.contains(expected), "{logged}");
        assert!(!logged.contains("user:"), "{logged}");
    }
}

#[tokio::test]
async fn the_config_warning_names_a_repo_url_with_credentials_without_showing_them() {
    let CredentialsWarning { banner, logged, .. } =
        write_config_with_credentials(&config_tracking(&[
            GOOD_REPO,
            "https://user:secretpw@example.com/foo/bar",
        ]))
        .await;

    assert!(
        banner.contains("must not contain credentials"),
        "why: {banner}"
    );
    assert!(
        banner.contains("example.com/foo/bar"),
        "which repo: {banner}"
    );
    assert!(logged.contains("must not contain credentials"), "{logged}");
}

#[tokio::test]
async fn metrics_flag_tracked_repos_and_keep_reporting_untracked_archives() {
    let (forge, upstream) = forge_with("foo/bar").await;
    let never = "https://example.com/never/fetched";
    let app = TestApp::tracking(&[upstream.url(), never]).await;
    let start = app.now();
    app.advance_and_settle(12 * HOUR).await;
    let id = format!("http://{}/foo/bar", forge.host());
    let tracked_flag = |metrics: &common::Metrics, repo: &str| {
        metrics.value("archiver_repo_tracked", &[("repo", repo)])
    };

    let metrics = app.metrics().await;
    assert_eq!(tracked_flag(&metrics, &id), Some(1.0));
    assert_eq!(tracked_flag(&metrics, never), Some(1.0));

    untrack(&app, upstream.url(), "Keep archive").await;

    let metrics = app.metrics().await;
    assert_eq!(tracked_flag(&metrics, &id), Some(0.0));
    assert_eq!(tracked_flag(&metrics, never), Some(1.0));
    assert_eq!(metrics.repo_status(&id), Some("available"));
    assert_eq!(metrics.last_success(&id), Some(start + 12 * 3600));
    let size = metrics.value("archiver_repo_size_bytes", &[("repo", &id)]);
    assert!(size.is_some_and(|bytes| bytes > 0.0), "{size:?}");
}

#[tokio::test]
async fn untracking_a_repo_stops_its_fetch_in_progress() {
    let (forge, upstream) = forge_with("foo/bar").await;
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    forge.hang("foo/bar");
    app.advance(24 * HOUR);
    forge.wait_for_hung_request().await;

    let page = tokio::time::timeout(
        Duration::from_secs(10),
        untrack(&app, upstream.url(), "Delete archive"),
    )
    .await
    .expect("untracking doesn't wait for the fetch");

    assert_eq!(page.status, 200, "{}", page.text());
    assert!(!archive.exists());
    let state = std::fs::read_to_string(app.data_dir().join("state.json")).unwrap();
    assert!(!state.contains("foo/bar"), "{state}");
}

#[tokio::test]
async fn untracking_removes_the_repo_from_the_config_and_leaves_the_rest_of_the_file_alone() {
    let x = "https://example.com/foo/x";
    let (a, b) = ("https://example.com/foo/a", "https://example.com/foo/b");
    let default_config = TestApp::start().await.config();
    let cases = [
        (
            default_config.replace(
                "  # \"https://github.com/owner/repo\",\n",
                &format!("  # \"https://github.com/owner/repo\",\n  \"{x}\",\n"),
            ),
            default_config.clone(),
        ),
        (
            format!(
                "# My archive\ninterval = \"12h\" # twice a day\n\nrepos = [\n  \"{a}\", # the first one\n  # \"https://example.com/old/one\",\n  \"{x}\", # remove me\n  \"{b}\",\n]\n\n[hosts.\"github.com\"]\ntoken = \"ghp_x\" # keep me\n"
            ),
            format!(
                "# My archive\ninterval = \"12h\" # twice a day\n\nrepos = [\n  \"{a}\", # the first one\n  # \"https://example.com/old/one\",\n  \"{b}\",\n]\n\n[hosts.\"github.com\"]\ntoken = \"ghp_x\" # keep me\n"
            ),
        ),
        (
            format!("repos = [\n  \"{x}\", # first\n  \"{a}\",\n]\n"),
            format!("repos = [\n  \"{a}\",\n]\n"),
        ),
        (
            format!("repos = [\n    \"{a}\",\n    \"{x}\"\n]\n"),
            format!("repos = [\n    \"{a}\"\n]\n"),
        ),
        (
            format!("repos = [\"{a}\", \"{x}\", \"{b}\"] # one line\n"),
            format!("repos = [\"{a}\", \"{b}\"] # one line\n"),
        ),
        (
            format!("repos = [\"{x}\", \"{a}\"]\n"),
            format!("repos = [\"{a}\"]\n"),
        ),
        (
            format!("repos = [\"{a}\", \"{x}\"]\n"),
            format!("repos = [\"{a}\"]\n"),
        ),
        (format!("repos = [\"{x}\"]\n"), "repos = []\n".to_owned()),
        (
            format!("repos = [\n  \"{x}\",\n]\n"),
            "repos = []\n".to_owned(),
        ),
        (
            format!(
                "repos = [\n  \"{x}\",\n  \"{a}\",\n  \"HTTPS://EXAMPLE.com/foo/x.git/\",\n]\n"
            ),
            format!("repos = [\n  \"{a}\",\n]\n"),
        ),
        (
            format!("repos = [\r\n  \"{a}\",\r\n  \"{x}\",\r\n]\r\n"),
            format!("repos = [\r\n  \"{a}\",\r\n]\r\n"),
        ),
    ];
    for (before, after) in cases {
        let app = TestApp::with_config(&before).await;
        let page = untrack(&app, x, "Keep archive").await;
        assert_eq!(page.status, 200, "{before:?}: {}", page.text());
        assert_eq!(app.config(), after, "from {before:?}");
        assert!(!tracked(&app).await.contains(&x.to_owned()), "{before:?}");
    }
}

#[tokio::test]
async fn each_tracked_card_offers_to_untrack_after_asking_what_to_do_with_the_archive() {
    let app = TestApp::tracking(&["https://github.com/foo/bar"]).await;

    let html = Html::parse_fragment(&card(&app, "https://github.com/foo/bar").await);
    let opener = select(&html, "button[data-dialog]");
    assert_eq!(opener.iter().map(text).collect::<Vec<_>>(), ["Untrack"]);
    let dialog_id = opener[0].value().attr("data-dialog").unwrap();
    let dialog = &select(&html, &format!("dialog#{dialog_id}"))[0];
    assert_eq!(
        text(&select_in(dialog, "h2")[0]),
        "Untrack “github.com/foo/bar”?"
    );
    assert_eq!(
        text(&select_in(dialog, "p")[0]),
        "The app stops fetching it. You can keep the archive or delete it from disk; \
         deleting can’t be undone."
    );
    let buttons: Vec<String> = select_in(dialog, "button").iter().map(text).collect();
    assert_eq!(buttons, ["Cancel", "Keep archive", "Delete archive"]);
    let delete = &select_in(dialog, "button.danger")[0];
    assert_eq!(text(delete), "Delete archive");
}

#[tokio::test]
async fn untracked_archives_are_read_only() {
    let (forge, upstream) = forge_with("foo/bar").await;
    let app = TestApp::tracking(&[upstream.url(), "https://example.com/other/repo"]).await;
    app.advance_and_settle(12 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    let id = format!("http://{}/foo/bar", forge.host());
    let form = [("repo", id.as_str()), ("archive", "delete")];

    untrack(&app, upstream.url(), "Keep archive").await;
    let html = Html::parse_fragment(&card(&app, &id).await);
    assert!(select(&html, "button[data-dialog]").is_empty());
    assert!(select(&html, "dialog").is_empty());
    assert!(!select(&html, ".clone-line").is_empty(), "still cloneable");

    let page = app.post_form("/repos/untrack", &form).await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert!(archive.exists(), "an untracked archive isn't deleted");
    assert_eq!(untracked(&app).await, [id]);
}

#[tokio::test]
async fn untracking_while_the_config_is_invalid_says_so_and_changes_nothing() {
    let url = "https://example.com/foo/bar";
    let app = TestApp::tracking(&[url]).await;
    let broken = "repos = [\n";
    let html = Html::parse_fragment(&card(&app, url).await);
    let repo = select(&html, "dialog input[name=repo]")[0]
        .value()
        .attr("value")
        .unwrap()
        .to_owned();
    app.write_config(broken);

    let page = app
        .post_form("/repos/untrack", &[("repo", &repo), ("archive", "keep")])
        .await;

    assert_eq!(page.status, 422);
    let html = Html::parse_document(&page.text());
    let alert = text(&select(&html, "main [role=alert]")[0]);
    assert!(alert.contains("Couldn’t untrack the repo"), "{alert}");
    assert!(alert.contains("invalid"), "{alert}");
    assert_eq!(app.config(), broken);
    assert_eq!(tracked(&app).await, [url]);
}

#[tokio::test]
async fn deleting_an_archive_never_waits_for_another_repos_fetch() {
    let forge = FakeForge::start().await;
    let stuck = forge.create_repo("a/stuck");
    stuck.commit("first");
    let gone = forge.create_repo("a/gone");
    gone.commit("first");
    let app = TestApp::tracking(&[gone.url(), stuck.url()]).await;
    app.advance_and_settle(12 * HOUR).await;
    let archive = app.archive(&format!("{}/a/gone", forge.host()));
    assert!(archive.exists());
    forge.hang("a/stuck");
    app.advance(12 * HOUR);
    forge.wait_for_hung_request().await;

    let page = tokio::time::timeout(
        Duration::from_secs(10),
        untrack(&app, gone.url(), "Delete archive"),
    )
    .await
    .expect("deleting doesn't wait for the other fetch");

    assert_eq!(page.status, 200, "{}", page.text());
    assert!(!archive.exists());
}
