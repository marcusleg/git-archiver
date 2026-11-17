//! Adding a repo through the list page's form.

mod common;

use common::app_page::{AppPage, PANE_CARD, select, select_in, text};
use common::forge::{DMCA_MESSAGE, DMCA_NOTICE, Failure, FakeForge};
use common::{HOUR, Page, TestApp, last_successes, three_repos, urls};
use scraper::{ElementRef, Html};

/// The add form's error message, if it shows one. Only an open dialog's
/// error counts: a closed one isn't seen.
fn form_error(page: &Page) -> Option<String> {
    let html = Html::parse_document(&page.text());
    select(&html, "dialog[open] form [role=alert]")
        .first()
        .map(text)
}

/// The URL in the add form's field, if its dialog is open.
fn form_value(page: &Page) -> Option<String> {
    let html = Html::parse_document(&page.text());
    select(&html, "dialog[open] form input[name=url]")
        .first()
        .and_then(|input| input.value().attr("value").map(str::to_owned))
}

/// The "+ Add repo" buttons in `scope`, outside the dialog, each checked to
/// open the add dialog.
fn add_buttons<'a>(page: &AppPage, scope: &ElementRef<'a>) -> Vec<ElementRef<'a>> {
    let dialog_id = page
        .add_dialog()
        .value()
        .attr("id")
        .expect("the dialog has an id")
        .to_owned();
    let in_dialog = |button: &ElementRef| {
        button.ancestors().any(|node| {
            node.value()
                .as_element()
                .is_some_and(|e| e.name() == "dialog")
        })
    };
    let buttons: Vec<_> = select_in(scope, "button")
        .into_iter()
        .filter(|button| text(button) == "Add repo" && !in_dialog(button))
        .collect();
    for button in &buttons {
        assert_eq!(button.value().attr("type"), Some("button"));
        assert_eq!(button.value().attr("commandfor"), Some(dialog_id.as_str()));
        assert_eq!(button.value().attr("command"), Some("show-modal"));
        assert_eq!(button.value().attr("aria-haspopup"), Some("dialog"));
    }
    buttons
}

/// Each tracked repo's row: its configured URL and status, in page order.
async fn rows(app: &TestApp) -> Vec<(String, String)> {
    app.list_page()
        .await
        .tracked()
        .into_iter()
        .map(|row| (row.title, row.status))
        .collect()
}

#[tokio::test]
async fn the_heading_and_the_empty_state_each_have_an_add_button_opening_the_dialog() {
    let app = TestApp::start().await;
    let page = app.list_page().await;
    let html = &page.html;
    let empty = select(html, "main .card.empty")[0];
    assert_eq!(add_buttons(&page, &empty).len(), 1, "in the empty state");
    assert_eq!(add_buttons(&page, &select(html, "main")[0]).len(), 1);

    let app = TestApp::tracking(&["https://github.com/foo/bar"]).await;
    let page = app.list_page().await;
    let html = &page.html;
    let heading = select(html, "main section .section-head:has(#tracked-heading)")[0];
    assert_eq!(add_buttons(&page, &heading).len(), 1, "beside the heading");
    assert_eq!(add_buttons(&page, &select(html, "main")[0]).len(), 1);
}

#[tokio::test]
async fn the_add_dialog_is_closed_and_holds_the_form() {
    for app in [
        TestApp::start().await,
        TestApp::tracking(&["https://github.com/foo/bar"]).await,
    ] {
        let response = app.get("/").await;
        let page = AppPage::parse(&response.text());
        let html = &page.html;
        let dialog = page.add_dialog();
        assert_eq!(dialog.value().attr("open"), None, "closed until asked for");

        let heading = &select_in(&dialog, "h2")[0];
        assert_eq!(text(heading), "Add a repo");
        assert_eq!(
            dialog.value().attr("aria-labelledby"),
            heading.value().attr("id")
        );
        assert_eq!(
            select_in(&dialog, "p").iter().map(text).collect::<Vec<_>>(),
            ["The app clones it right away, then fetches it once per interval."]
        );
        let form = page.add_form();
        assert!(
            form.ancestors().any(|node| node.id() == dialog.id()),
            "the form is inside the add dialog"
        );
        assert_eq!(form.value().attr("action"), Some("/repos"));
        let input = &select_in(&form, "input[name=url]")[0];
        let id = input.value().attr("id").unwrap();
        assert_eq!(
            text(&select(html, &format!("label[for={id}]"))[0]),
            "Repo URL"
        );
        assert_eq!(
            select_in(&form, "button")
                .iter()
                .map(text)
                .collect::<Vec<_>>(),
            ["Cancel", "Add repo"]
        );
        assert_eq!(form_error(&response), None);
    }
}

#[tokio::test]
async fn an_added_repo_is_cloned_right_away_and_shows_as_available() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let tip = upstream.commit("first");
    let app = TestApp::start().await;
    let start = app.now();

    let page = app.add_repo(upstream.url()).await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_eq!(page.path, "/");
    assert_eq!(form_error(&page), None);
    let landed = AppPage::parse(&page.text());
    assert_eq!(
        landed.titles("Tracked repos"),
        [upstream.url()],
        "lands on the list with the new repo in it"
    );
    app.settle().await;

    assert_eq!(app.now(), start, "the clock didn't move");
    assert_eq!(
        rows(&app).await,
        [(upstream.url().to_owned(), "Available".to_owned())]
    );
    let html = Html::parse_document(&app.get("/").await.text());
    let status = select(&html, &format!("{PANE_CARD} .status.available"));
    assert_eq!(
        status.iter().map(text).collect::<Vec<_>>(),
        ["Fetched just now"]
    );
    let id = format!("http://{}/foo/bar", forge.host());
    assert_eq!(app.metrics().await.last_success(&id), Some(start));
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    assert_eq!(common::git::refs(&archive)["refs/heads/main"], tip);
}

#[tokio::test]
async fn adding_a_repo_appends_it_to_the_config_and_leaves_the_rest_of_the_file_alone() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/new");
    upstream.commit("first");
    let new = upstream.url();
    let existing = "https://example.com/foo/existing";

    let default_config = TestApp::start().await.config();
    let cases = [
        (
            default_config.clone(),
            default_config.replace(
                "  # \"https://github.com/owner/repo\",\n",
                &format!("  # \"https://github.com/owner/repo\",\n  \"{new}\",\n"),
            ),
        ),
        (
            format!(
                "# My archive\ninterval = \"12h\" # twice a day\n\nrepos = [\n  \"{existing}\", # the first one\n  # \"https://example.com/old/one\",\n]\n\n[hosts.\"github.com\"]\ntoken = \"ghp_x\" # keep me\n"
            ),
            format!(
                "# My archive\ninterval = \"12h\" # twice a day\n\nrepos = [\n  \"{existing}\", # the first one\n  # \"https://example.com/old/one\",\n  \"{new}\",\n]\n\n[hosts.\"github.com\"]\ntoken = \"ghp_x\" # keep me\n"
            ),
        ),
        (
            format!("repos = [\n    \"{existing}\"\n]\n"),
            format!("repos = [\n    \"{existing}\",\n    \"{new}\",\n]\n"),
        ),
        (
            format!("repos = [\"{existing}\"] # one line\n"),
            format!("repos = [\"{existing}\", \"{new}\"] # one line\n"),
        ),
        ("repos = []\n".to_owned(), format!("repos = [\"{new}\"]\n")),
        (
            "interval = \"6h\"\n".to_owned(),
            format!("interval = \"6h\"\nrepos = [\"{new}\"]\n"),
        ),
        (
            "# My archive\n\n[hosts.\"github.com\"]\ntoken = \"ghp_x\"\n".to_owned(),
            format!(
                "# My archive\n\nrepos = [\"{new}\"]\n\n[hosts.\"github.com\"]\ntoken = \"ghp_x\"\n"
            ),
        ),
        (
            format!("# Windows\r\nrepos = [\r\n  \"{existing}\",\r\n]\r\ninterval = \"6h\"\r\n"),
            format!(
                "# Windows\r\nrepos = [\r\n  \"{existing}\",\r\n  \"{new}\",\r\n]\r\ninterval = \"6h\"\r\n"
            ),
        ),
    ];
    for (before, expected) in cases {
        let app = TestApp::with_config(&before).await;
        let page = app.add_repo(new).await;
        assert_eq!(form_error(&page), None, "config:\n{before}");
        assert_eq!(app.config(), expected, "config:\n{before}");
    }
}

#[tokio::test]
async fn the_config_is_replaced_in_one_step_never_rewritten_in_place() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    let app = TestApp::start().await;
    let before = app.config();
    // A second name for the old file. Writing in place would change what it
    // shows; replacing the file leaves it as it was.
    let old = app.data_dir().join("config.toml.old");
    std::fs::hard_link(app.config_path(), &old).unwrap();

    app.add_repo(upstream.url()).await;
    app.settle().await;

    assert_eq!(std::fs::read_to_string(&old).unwrap(), before);
    assert!(app.config().contains(upstream.url()));
    let mut files: Vec<String> = std::fs::read_dir(app.data_dir())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != "repos")
        .collect();
    files.sort();
    assert_eq!(files, ["config.toml", "config.toml.old", "state.json"]);
}

#[tokio::test]
async fn an_unreachable_repo_is_not_added_and_the_form_shows_gits_error() {
    let forge = FakeForge::start().await;
    forge.fail("foo/gone", Failure::NotFound);
    forge.fail("foo/blocked", Failure::DmcaTakedown);
    let gone = format!("http://{}/foo/gone.git", forge.host());
    let blocked = format!("http://{}/foo/blocked.git", forge.host());
    let app = TestApp::start().await;
    let config = app.config();

    let cases = [
        (
            gone.clone(),
            format!("Couldn’t reach “{gone}”: Repository not found."),
        ),
        (
            blocked.clone(),
            format!(
                "Couldn’t reach “{blocked}”: {DMCA_MESSAGE} \
                 See the takedown notice for more details: {DMCA_NOTICE}"
            ),
        ),
    ];
    for (url, expected) in cases {
        let page = app.add_repo(&url).await;
        assert_eq!(page.status, 422);
        assert_eq!(form_error(&page).as_deref(), Some(expected.as_str()));
        assert_eq!(form_value(&page).as_deref(), Some(url.as_str()));
    }

    let unresolvable = "http://nonexistent.invalid/foo/bar";
    let page = app.add_repo(unresolvable).await;
    assert_eq!(page.status, 422);
    let error = form_error(&page).unwrap();
    assert!(
        error.starts_with(&format!("Couldn’t reach “{unresolvable}”: "))
            && error.contains("nonexistent.invalid"),
        "{error}"
    );

    assert_eq!(app.config(), config);
    assert!(rows(&app).await.is_empty());
}

#[tokio::test]
async fn a_url_with_the_identity_of_a_tracked_repo_is_rejected_as_a_duplicate() {
    let app = TestApp::tracking(&["https://github.com/foo/bar"]).await;
    let config = app.config();

    let page = app.add_repo("https://GitHub.com/foo/bar.git/").await;
    assert_eq!(page.status, 422);
    assert_eq!(
        form_error(&page).as_deref(),
        Some(
            "“https://GitHub.com/foo/bar.git/” is already tracked as “https://github.com/foo/bar”."
        )
    );

    let page = app.add_repo("https://github.com/foo/bar").await;
    assert_eq!(
        form_error(&page).as_deref(),
        Some("“https://github.com/foo/bar” is already tracked.")
    );
    assert_eq!(app.config(), config);
}

#[tokio::test]
async fn a_url_differing_only_in_scheme_is_rejected_because_it_shares_the_archive() {
    let app = TestApp::tracking(&["https://github.com/foo/bar"]).await;
    let config = app.config();

    let page = app.add_repo("http://github.com/foo/bar").await;

    assert_eq!(
        form_error(&page).as_deref(),
        Some("“http://github.com/foo/bar” is already tracked as “https://github.com/foo/bar”.")
    );
    assert_eq!(app.config(), config);
}

#[tokio::test]
async fn an_upstream_that_doesnt_answer_within_a_minute_is_not_added() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/slow");
    upstream.commit("first");
    forge.hang("foo/slow");
    let app = TestApp::start().await;
    let config = app.config();

    let (page, ()) = tokio::join!(app.add_repo(upstream.url()), async {
        forge.wait_for_hung_request().await;
        app.advance(std::time::Duration::from_secs(60));
    });

    assert_eq!(page.status, 422);
    assert_eq!(
        form_error(&page),
        Some(format!(
            "Couldn’t reach “{}”: it didn’t answer within a minute.",
            upstream.url()
        ))
    );
    assert_eq!(app.config(), config);
}

#[tokio::test]
async fn a_url_the_config_would_reject_is_not_added() {
    let app = TestApp::start().await;
    let config = app.config();

    let cases = [
        ("", "Enter the URL of the repo to add."),
        ("   ", "Enter the URL of the repo to add."),
        ("not a url", "“not a url” is not a valid URL."),
        (
            "ftp://example.com/foo/bar",
            "“ftp://example.com/foo/bar” must be an http:// or https:// URL.",
        ),
        (
            "https://user:secret@example.com/foo/bar",
            "“https://user:secret@example.com/foo/bar” must not contain credentials; \
             set a token under [hosts.\"<host>\"] instead.",
        ),
        (
            "https://example.com/",
            "“https://example.com/” has no repository path.",
        ),
        (
            "https://example.com/foo.git/bar",
            "“https://example.com/foo.git/bar” has an unsupported repository path.",
        ),
    ];
    for (url, expected) in cases {
        let page = app.add_repo(url).await;
        assert_eq!(page.status, 422, "{url:?}");
        assert_eq!(form_error(&page).as_deref(), Some(expected), "{url:?}");
    }
    assert_eq!(app.config(), config);
}

#[tokio::test]
async fn surrounding_whitespace_is_trimmed_from_the_url() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    let app = TestApp::with_config("repos = []\n").await;

    let page = app.add_repo(&format!("  {}\n", upstream.url())).await;

    assert_eq!(form_error(&page), None);
    assert_eq!(app.config(), format!("repos = [\"{}\"]\n", upstream.url()));
}

#[tokio::test]
async fn nothing_is_added_while_the_config_file_is_invalid() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    let app = TestApp::start().await;
    let broken = "repos = [\n";
    app.write_config(broken);

    let page = app.add_repo(upstream.url()).await;

    assert_eq!(page.status, 422);
    let error = form_error(&page).unwrap();
    assert!(
        error.starts_with("Couldn’t add the repo: the config file is invalid."),
        "{error}"
    );
    assert_eq!(app.config(), broken);
}

#[tokio::test]
async fn adding_a_repo_counts_as_its_first_attempt_and_the_gap_reflects_the_new_count() {
    let forge = FakeForge::start().await;
    let (repos, ids) = three_repos(&forge);
    let app = TestApp::tracking(&urls(&repos[..2])).await;
    let start = app.now();
    let h = |hours: u64| Some(start + hours * 3600);
    app.advance_and_settle(12 * HOUR).await;
    app.advance_and_settle(2 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [h(12), None, None]);

    app.add_repo(repos[2].url()).await;
    app.settle().await;

    assert_eq!(last_successes(&app, &ids).await, [h(12), None, h(14)]);
    assert_eq!(app.next_fetch().await, h(22), "24h / 3 after the clone");

    app.advance_and_settle(8 * HOUR).await;
    assert_eq!(last_successes(&app, &ids).await, [h(12), h(22), h(14)]);
}

#[tokio::test]
async fn the_clone_of_an_added_repo_waits_for_a_fetch_in_progress() {
    let forge = FakeForge::start().await;
    let stuck = forge.create_repo("a/stuck");
    stuck.commit("first");
    let new = forge.create_repo("a/new");
    new.commit("first");
    forge.hang("a/stuck");
    let app = TestApp::tracking(&[stuck.url()]).await;
    let start = app.now();
    let new_id = format!("http://{}/a/new", forge.host());

    app.advance(24 * HOUR);
    forge.wait_for_hung_request().await;
    let page = app.add_repo(new.url()).await;
    assert_eq!(form_error(&page), None);

    assert_eq!(app.metrics().await.last_success(&new_id), None);
    assert!(!app.archive(&format!("{}/a/new", forge.host())).exists());

    app.advance_and_settle(HOUR).await;
    assert_eq!(
        app.metrics().await.last_success(&new_id),
        Some(start + 25 * 3600),
        "cloned as soon as the stuck fetch timed out"
    );
}
