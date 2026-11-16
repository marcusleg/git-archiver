//! Unavailable and Fetch failed: how refusals and failures upstream show on
//! the list page, in `/metrics` and in the logs, and that they never touch
//! the archive.

mod common;

use common::forge::{DMCA_MESSAGE, DMCA_NOTICE, Failure, FakeForge, Upstream};
use common::git::{has_object, refs};
use std::path::PathBuf;

use common::{HOUR, TestApp, logs};
use scraper::{ElementRef, Html, Selector};

fn select<'a>(html: &'a Html, selector: &str) -> Vec<ElementRef<'a>> {
    html.select(&Selector::parse(selector).unwrap()).collect()
}

fn text(element: &ElementRef) -> String {
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// What the only card on the list page shows about the repo's status.
struct StatusView {
    /// The status line, whitespace collapsed.
    line: String,
    /// The `class` of the status line's icon.
    icon: String,
    /// The text under the status line, as written, if any.
    reason: Option<String>,
    /// The `datetime` of the status line's `<time>`, if any.
    since: Option<String>,
}

async fn status_view(app: &TestApp) -> StatusView {
    let html = Html::parse_document(&app.get("/").await.text());
    let status = &select(&html, "main article .status")[0];
    let icon = status
        .select(&Selector::parse("svg").unwrap())
        .next()
        .unwrap_or_else(|| panic!("status has an icon: {}", status.html()));
    StatusView {
        line: text(status),
        icon: icon.value().attr("class").unwrap_or_default().to_owned(),
        reason: select(&html, "main article .reason")
            .first()
            .map(|e| e.text().collect()),
        since: status
            .select(&Selector::parse("time").unwrap())
            .next()
            .and_then(|t| t.value().attr("datetime").map(str::to_owned)),
    }
}

/// A repo archived by the app.
struct Archived {
    upstream: Upstream,
    app: TestApp,
    /// The repo identity.
    id: String,
    archive: PathBuf,
}

/// A repo with a commit and a tag, archived by the app 24 hours after start
/// (2026-11-17 12:00 UTC). Upstream then gets another commit, so a fetch
/// that went through would change the archive.
async fn archived_repo(forge: &FakeForge) -> Archived {
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    upstream.tag("v1");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    upstream.commit("second");
    Archived {
        id: format!("http://{}/foo/bar", forge.host()),
        archive: app.archive(&format!("{}/foo/bar", forge.host())),
        upstream,
        app,
    }
}

/// Refuses the archived repo the given way at the next fetch (2026-11-18
/// 12:00 UTC) and checks it is Unavailable with git's message, leaving the
/// archive as it was.
async fn assert_refusal_marks_unavailable(failure: Failure, http_status: u16, message: &[&str]) {
    let forge = FakeForge::start().await;
    let Archived {
        app, id, archive, ..
    } = archived_repo(&forge).await;
    let before = refs(&archive);

    forge.fail("foo/bar", failure);
    app.advance_and_settle(24 * HOUR).await;

    assert_eq!(refs(&archive), before, "the archive is untouched");
    let view = status_view(&app).await;
    assert_eq!(
        view.line,
        format!("Unavailable since 18 Nov 2026 · HTTP {http_status}")
    );
    assert!(view.icon.contains("icon-ban"), "{}", view.icon);
    let reason = view.reason.expect("git's message under the status");
    for part in message {
        assert!(reason.contains(part), "{part:?} in {reason:?}");
    }
    let metrics = app.metrics().await;
    assert_eq!(metrics.repo_status(&id), Some("unavailable"));
    assert_eq!(metrics.fetch_errors("unavailable"), Some(1.0));
    assert_eq!(metrics.fetch_errors("fetch_failed"), Some(0.0));
    assert_eq!(
        metrics.last_success(&id),
        Some(app.now() - 24 * 3600),
        "the last success stays"
    );
}

#[tokio::test]
async fn a_dmca_takedown_marks_the_repo_unavailable_with_githubs_message() {
    assert_refusal_marks_unavailable(
        Failure::DmcaTakedown,
        403,
        &[
            &format!("remote: {DMCA_MESSAGE}\n"),
            &format!("remote:   {DMCA_NOTICE}\n"),
            "The requested URL returned error: 403",
        ],
    )
    .await;
}

#[tokio::test]
async fn a_repo_not_found_upstream_is_unavailable() {
    assert_refusal_marks_unavailable(
        Failure::NotFound,
        404,
        &["remote: Repository not found.\n", "not found"],
    )
    .await;
}

#[tokio::test]
async fn an_upstream_asking_for_credentials_marks_the_repo_unavailable() {
    assert_refusal_marks_unavailable(
        Failure::Unauthorized,
        401,
        &["could not read Username", "terminal prompts disabled"],
    )
    .await;
}

/// Checks the archived repo is marked Fetch failed with the error, leaving
/// the archive as it was.
async fn assert_fetch_failed(app: &TestApp, error: &str) {
    let view = status_view(app).await;
    assert_eq!(view.line, "Fetch failed");
    assert!(view.icon.contains("icon-refresh-cw-off"), "{}", view.icon);
    let reason = view.reason.expect("the error under the status");
    assert!(reason.contains(error), "{error:?} in {reason:?}");
    let metrics = app.metrics().await;
    assert_eq!(metrics.fetch_errors("fetch_failed"), Some(1.0));
    assert_eq!(metrics.fetch_errors("unavailable"), Some(0.0));
}

#[tokio::test]
async fn a_server_error_upstream_is_a_failed_fetch() {
    let forge = FakeForge::start().await;
    let Archived {
        app, id, archive, ..
    } = archived_repo(&forge).await;
    let before = refs(&archive);

    forge.fail("foo/bar", Failure::ServerError);
    app.advance_and_settle(24 * HOUR).await;

    assert_eq!(refs(&archive), before, "the archive is untouched");
    assert_fetch_failed(&app, "The requested URL returned error: 500").await;
    assert_eq!(app.metrics().await.repo_status(&id), Some("fetch_failed"));
}

#[tokio::test]
async fn a_refused_connection_is_a_failed_fetch() {
    let forge = FakeForge::start().await;
    let Archived {
        app, id, archive, ..
    } = archived_repo(&forge).await;
    let before = refs(&archive);

    forge.go_offline().await;
    app.advance_and_settle(24 * HOUR).await;

    assert_eq!(refs(&archive), before, "the archive is untouched");
    assert_fetch_failed(&app, "Failed to connect").await;
    assert_eq!(app.metrics().await.repo_status(&id), Some("fetch_failed"));
}

#[tokio::test]
async fn a_fetch_hanging_past_the_timeout_is_a_failed_fetch() {
    let forge = FakeForge::start().await;
    let Archived {
        app, id, archive, ..
    } = archived_repo(&forge).await;
    let before = refs(&archive);

    forge.hang("foo/bar");
    app.advance(24 * HOUR);
    forge.wait_for_hung_request().await;
    app.advance_and_settle(HOUR).await;

    assert_eq!(refs(&archive), before, "the archive is untouched");
    assert_fetch_failed(&app, "timed out after 1h").await;
    assert_eq!(app.metrics().await.repo_status(&id), Some("fetch_failed"));
}

#[tokio::test]
async fn a_repo_refused_on_its_first_fetch_gets_no_archive() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    forge.fail("foo/bar", Failure::DmcaTakedown);
    let app = TestApp::tracking(&[upstream.url()]).await;

    app.advance_and_settle(24 * HOUR).await;

    assert!(!app.archive(&format!("{}/foo/bar", forge.host())).exists());
    let view = status_view(&app).await;
    assert_eq!(view.line, "Unavailable since 17 Nov 2026 · HTTP 403");
}

#[tokio::test]
async fn unavailable_since_keeps_the_time_of_the_first_refusal() {
    let forge = FakeForge::start().await;
    let Archived { mut app, id, .. } = archived_repo(&forge).await;
    forge.fail("foo/bar", Failure::DmcaTakedown);
    app.advance_and_settle(24 * HOUR).await;

    app.restart().await;
    forge.fail("foo/bar", Failure::NotFound);
    app.advance_and_settle(24 * HOUR).await;
    app.advance_and_settle(24 * HOUR).await;

    let view = status_view(&app).await;
    assert_eq!(view.line, "Unavailable since 18 Nov 2026 · HTTP 404");
    assert_eq!(view.since.as_deref(), Some("2026-11-18T12:00:00Z"));
    assert!(
        view.reason.unwrap().contains("Repository not found."),
        "the reason is the latest refusal's"
    );
    let metrics = app.metrics().await;
    assert_eq!(metrics.repo_status(&id), Some("unavailable"));
    assert_eq!(
        metrics.fetch_errors("unavailable"),
        Some(2.0),
        "counted since the restart"
    );
}

#[tokio::test]
async fn a_restored_repo_is_available_again_and_keeps_its_pre_takedown_history() {
    let forge = FakeForge::start().await;
    let Archived {
        upstream,
        app,
        id,
        archive,
    } = archived_repo(&forge).await;
    let old_tip = refs(&archive)["refs/heads/main"].clone();
    forge.fail("foo/bar", Failure::DmcaTakedown);
    app.advance_and_settle(24 * HOUR).await;
    assert_eq!(app.metrics().await.repo_status(&id), Some("unavailable"));

    let new_tip = upstream.force_push_new_history("rewritten during the takedown");
    forge.restore("foo/bar");
    app.advance_and_settle(24 * HOUR).await;

    let view = status_view(&app).await;
    assert_eq!(view.line, "Fetched just now");
    assert!(view.icon.contains("icon-circle-check"), "{}", view.icon);
    assert_eq!(view.reason, None);
    let metrics = app.metrics().await;
    assert_eq!(metrics.repo_status(&id), Some("available"));
    assert_eq!(metrics.last_success(&id), Some(app.now()));
    assert!(
        refs(&archive).values().any(|sha| *sha == old_tip),
        "the pre-takedown tip is still a ref: {:?}",
        refs(&archive)
    );
    assert!(has_object(&archive, &new_tip));
}

#[tokio::test]
async fn every_status_change_logs_one_line_with_the_old_and_new_status_and_the_reason() {
    let logs = logs::capture();
    let forge = FakeForge::start().await;
    let Archived { app, id, .. } = archived_repo(&forge).await;

    forge.fail("foo/bar", Failure::DmcaTakedown);
    app.advance_and_settle(24 * HOUR).await;
    app.advance_and_settle(24 * HOUR).await;
    forge.fail("foo/bar", Failure::ServerError);
    app.advance_and_settle(24 * HOUR).await;
    forge.restore("foo/bar");
    app.advance_and_settle(24 * HOUR).await;

    let changes = logs.matching("status changed");
    assert_eq!(changes.len(), 4, "{changes:#?}");
    let expected = [
        ("from=none", "to=available", None),
        ("from=available", "to=unavailable", Some(DMCA_MESSAGE)),
        (
            "from=unavailable",
            "to=fetch_failed",
            Some("The requested URL returned error: 500"),
        ),
        ("from=fetch_failed", "to=available", None),
    ];
    for (line, (from, to, reason)) in changes.iter().zip(expected) {
        assert!(line.contains(&format!("repo={id}")), "{line}");
        assert!(line.contains(from), "{from} in {line}");
        assert!(line.contains(to), "{to} in {line}");
        if let Some(reason) = reason {
            assert!(line.contains("reason="), "{line}");
            assert!(line.contains(reason), "{reason} in {line}");
        }
    }
}
