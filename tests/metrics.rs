//! `/metrics`, as Prometheus scrapes it.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::forge::FakeForge;
use common::{HOUR, Metrics, TestApp};

/// Checks the Prometheus text exposition format (version 0.0.4): comment,
/// TYPE and sample lines only; each family typed once, before its samples,
/// and its samples grouped together.
fn assert_valid_exposition(text: &str) {
    let name_ok = |name: &str| {
        let mut chars = name.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == ':')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':')
    };
    assert!(text.ends_with('\n'), "ends with a newline");
    let mut typed = BTreeMap::new();
    let mut finished = BTreeSet::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# TYPE ") {
            let (name, kind) = rest.split_once(' ').expect("TYPE has a kind");
            assert!(name_ok(name), "bad metric name: {line}");
            assert!(
                ["counter", "gauge", "histogram", "summary", "untyped"].contains(&kind),
                "bad type: {line}"
            );
            assert!(
                typed.insert(name.to_owned(), kind.to_owned()).is_none(),
                "typed twice: {line}"
            );
        } else if let Some(rest) = line.strip_prefix("# HELP ") {
            let (name, _) = rest.split_once(' ').expect("HELP has text");
            assert!(name_ok(name), "bad metric name: {line}");
        } else if line.starts_with('#') {
        } else {
            assert!(!line.is_empty(), "no blank lines");
            let name = line.split(['{', ' ']).next().unwrap();
            assert!(name_ok(name), "bad metric name: {line}");
            assert!(typed.contains_key(name), "sample before its TYPE: {line}");
            if current.as_deref() != Some(name) {
                if let Some(previous) = current.replace(name.to_owned()) {
                    finished.insert(previous);
                }
                assert!(!finished.contains(name), "samples not grouped: {line}");
            }
        }
    }
    // Every sample line parses: name, quoted labels, float value.
    Metrics::parse(text);
}

#[tokio::test]
async fn metrics_report_status_last_success_size_next_fetch_and_fetch_duration() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url(), "https://example.com/never/fetched"]).await;
    let start = app.now();
    app.advance_and_settle(12 * HOUR).await;

    let page = app.get("/metrics").await;
    assert_eq!(page.status, 200);
    assert!(
        page.content_type.starts_with("text/plain; version=0.0.4"),
        "{}",
        page.content_type
    );
    let text = page.text();
    assert_valid_exposition(&text);
    let metrics = Metrics::parse(&text);

    let repo = format!("http://{}/foo/bar", forge.host());
    let labels = [("repo", repo.as_str())];
    assert_eq!(metrics.last_success(&repo), Some(start + 12 * 3600));
    let size = metrics.value("archiver_repo_size_bytes", &labels).unwrap();
    assert!(
        size > 1000.0,
        "a repo with a commit takes some space: {size}"
    );
    assert_eq!(metrics.next_fetch(), Some(start + 24 * 3600));
    let duration = metrics
        .value("archiver_fetch_duration_seconds", &[])
        .unwrap();
    assert!((0.0..60.0).contains(&duration), "{duration}");

    assert_eq!(metrics.repo_status(&repo), Some("available"));

    let never = [("repo", "https://example.com/never/fetched")];
    assert_eq!(
        metrics.last_success("https://example.com/never/fetched"),
        None
    );
    assert_eq!(metrics.value("archiver_repo_size_bytes", &never), None);
    assert_eq!(
        metrics.repo_status("https://example.com/never/fetched"),
        None
    );
    assert_eq!(metrics.value("archiver_repo_preserved_refs", &never), None);
    assert_eq!(
        metrics.value("archiver_repo_preserved_refs", &labels),
        Some(0.0)
    );
}

#[tokio::test]
async fn metrics_are_valid_before_anything_is_fetched() {
    let app = TestApp::start().await;

    let text = app.get("/metrics").await.text();
    assert_valid_exposition(&text);
    let metrics = Metrics::parse(&text);
    let samples: Vec<(&str, f64)> = metrics
        .samples()
        .iter()
        .map(|s| (s.name.as_str(), s.value))
        .collect();
    assert_eq!(
        samples,
        [
            ("archiver_fetch_errors_total", 0.0),
            ("archiver_fetch_errors_total", 0.0),
            ("archiver_rewrites_total", 0.0),
        ],
        "only the counters, at 0: {text}"
    );
    assert_eq!(metrics.fetch_errors("unavailable"), Some(0.0));
    assert_eq!(metrics.fetch_errors("fetch_failed"), Some(0.0));
    assert!(text.contains("# TYPE archiver_fetch_errors_total counter\n"));
}

#[tokio::test]
async fn metrics_count_preserved_refs_per_repo_and_rewrites() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    upstream.tag("v1");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let repo = format!("http://{}/foo/bar", forge.host());
    let labels = [("repo", repo.as_str())];
    let preserved = "archiver_repo_preserved_refs";

    let metrics = app.metrics().await;
    assert_eq!(metrics.value(preserved, &labels), Some(0.0));
    assert_eq!(metrics.value("archiver_rewrites_total", &[]), Some(0.0));

    upstream.force_push_new_history("rewritten");
    upstream.move_tag("v1");
    app.advance_and_settle(24 * HOUR).await;
    upstream.delete_tag("v1");
    app.advance_and_settle(24 * HOUR).await;
    app.advance_and_settle(24 * HOUR).await;

    let text = app.get("/metrics").await.text();
    assert_valid_exposition(&text);
    assert!(
        text.contains("# TYPE archiver_rewrites_total counter"),
        "{text}"
    );
    let metrics = Metrics::parse(&text);
    assert_eq!(metrics.value(preserved, &labels), Some(3.0));
    assert_eq!(metrics.value("archiver_rewrites_total", &[]), Some(2.0));
}

#[tokio::test]
async fn per_repo_series_follow_the_config_order_then_untracked_archives() {
    let app = TestApp::tracking(&[
        "https://example.com/zeta/one",
        "https://example.com/Alpha/two",
    ])
    .await;
    // Exact-string order, as on disk, not the list page's case-insensitive
    // order.
    for archive in ["example.com/beta/three", "example.com/Gamma/four"] {
        std::fs::create_dir_all(app.archive(archive)).unwrap();
    }

    let metrics = app.metrics().await;
    let repos: Vec<&str> = metrics
        .samples()
        .iter()
        .filter(|s| s.name == "archiver_repo_tracked")
        .map(|s| s.labels[0].1.as_str())
        .collect();
    assert_eq!(
        repos,
        [
            "https://example.com/zeta/one",
            "https://example.com/Alpha/two",
            "https://example.com/Gamma/four",
            "https://example.com/beta/three",
        ],
        "config order, not identity order"
    );
}
