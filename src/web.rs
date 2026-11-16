//! The web UI: server-rendered HTML, with the stylesheet and fonts embedded in
//! the binary so the page loads nothing from other hosts.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::archive::{Archives, Inspection};
use crate::clock::Clock;
use crate::metrics;
use crate::overview::{SharedOverview, Tracked};
use crate::state::{RepoState, Status};

pub struct AppState {
    pub config_path: PathBuf,
    pub clock: Arc<dyn Clock>,
    pub archives: Arc<Archives>,
    pub overview: SharedOverview,
}

const STYLESHEET: &str = include_str!("../assets/style.css");

const FONTS: &[(&str, &[u8])] = &[
    (
        "inter-latin-400.woff2",
        include_bytes!("../assets/fonts/inter-latin-400.woff2"),
    ),
    (
        "inter-latin-700.woff2",
        include_bytes!("../assets/fonts/inter-latin-700.woff2"),
    ),
    (
        "jetbrains-mono-latin-400.woff2",
        include_bytes!("../assets/fonts/jetbrains-mono-latin-400.woff2"),
    ),
];

const ASSET_CACHE: &str = "public, max-age=3600";

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(list_page))
        .route("/metrics", get(metrics))
        .route("/assets/style.css", get(stylesheet))
        .route("/assets/fonts/{name}", get(font))
        .with_state(state)
}

async fn metrics(State(state): State<Arc<AppState>>) -> Response {
    let body = metrics::render(&state.overview.lock().unwrap());
    ([(header::CONTENT_TYPE, metrics::CONTENT_TYPE)], body).into_response()
}

async fn stylesheet() -> Response {
    asset("text/css; charset=utf-8", STYLESHEET.as_bytes())
}

async fn font(Path(name): Path<String>) -> Response {
    match FONTS.iter().find(|(file, _)| *file == name) {
        Some((_, bytes)) => asset("font/woff2", bytes),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

fn asset(content_type: &'static str, body: &'static [u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, ASSET_CACHE),
        ],
        body,
    )
        .into_response()
}

async fn list_page(State(app): State<Arc<AppState>>) -> Html<String> {
    let tracked: Vec<(Tracked, RepoState)> = {
        let overview = app.overview.lock().unwrap();
        overview
            .tracked_with_state()
            .map(|(t, state)| (t.clone(), state.cloned().unwrap_or_default()))
            .collect()
    };
    let mut cards = Vec::with_capacity(tracked.len());
    for (repo, state) in tracked {
        let inspection = app.archives.inspect(&repo.id).await.unwrap_or_else(|e| {
            tracing::warn!(repo = %repo.id, error = %format!("{e:#}"), "couldn't read archive");
            Inspection::default()
        });
        cards.push((repo, state, inspection));
    }
    let now = app.clock.now();

    Html(
        layout(html! {
            @if cards.is_empty() {
                section .card.empty {
                    (icon_archive())
                    h2 { "No repos tracked yet" }
                    p {
                        "Add repo URLs to " code { "repos" } " in "
                        code { (app.config_path.display()) }
                        ". The app picks them up within a minute."
                    }
                }
            } @else {
                section aria-labelledby="tracked-heading" {
                    h2 #tracked-heading .section-label {
                        "Tracked repos " span .count { (cards.len()) }
                    }
                    ul .repos {
                        @for (repo, state, inspection) in &cards {
                            li { (repo_card(repo, state, inspection, now)) }
                        }
                    }
                }
            }
        })
        .into_string(),
    )
}

fn repo_card(
    repo: &Tracked,
    state: &RepoState,
    inspection: &Inspection,
    now: SystemTime,
) -> Markup {
    let fetched = state.status == Some(Status::Available);
    html! {
        article .card {
            h3 .repo-url { (repo.url) }
            @match (state.status, state.last_success) {
                (Some(Status::Available), Some(success)) => p .status.available {
                    (icon_circle_check())
                    span { "Fetched " (time(success, now)) }
                },
                _ => p .status {
                    (icon_clock())
                    span { "Not fetched yet" }
                },
            }
            dl .facts {
                div {
                    dt { "Last attempt" }
                    dd {
                        @match state.last_attempt {
                            Some(attempt) => (time(attempt, now)),
                            None => "Never",
                        }
                    }
                }
                div {
                    dt { "Default branch" }
                    dd {
                        @match &inspection.default_branch {
                            Some(branch) => code { (branch) },
                            None => span .muted { "—" },
                        }
                    }
                }
                div {
                    dt { "Tip" }
                    dd {
                        @match &inspection.tip {
                            Some(sha) => code .sha title=(sha) { (sha.get(..12).unwrap_or(sha)) },
                            None if fetched => span .muted { "No commits yet" },
                            None => span .muted { "—" },
                        }
                    }
                }
                div {
                    dt { "Size" }
                    dd .num {
                        @match state.size_bytes {
                            Some(bytes) => (format_size(bytes)),
                            None => span .muted { "—" },
                        }
                    }
                }
            }
        }
    }
}

/// A `<time>` showing how long ago `then` was, with the exact UTC time on
/// hover.
fn time(then: SystemTime, now: SystemTime) -> Markup {
    let exact = humantime::format_rfc3339_seconds(then).to_string();
    html! {
        time datetime=(exact) title=(exact.replace('T', " ").replace('Z', " UTC")) {
            (time_ago(then, now))
        }
    }
}

fn time_ago(then: SystemTime, now: SystemTime) -> String {
    let secs = now.duration_since(then).unwrap_or_default().as_secs();
    let (count, unit) = match secs {
        0..60 => return "just now".to_owned(),
        60..3600 => (secs / 60, "minute"),
        3600..86400 => (secs / 3600, "hour"),
        _ => (secs / 86400, "day"),
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("{count} {unit}{plural} ago")
}

/// Bytes in decimal units: "512 B", "1.2 kB", "3.4 MB".
fn format_size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = "B";
    for next in ["kB", "MB", "GB", "TB"] {
        if value < 1000.0 {
            break;
        }
        value /= 1000.0;
        unit = next;
    }
    format!("{value:.1} {unit}")
}

fn layout(content: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                meta name="color-scheme" content="dark";
                title { "Git Archiver" }
                // No product mark: an empty icon stops the browser asking for /favicon.ico.
                link rel="icon" href="data:,";
                link rel="stylesheet" href="/assets/style.css";
            }
            body {
                header .app-header {
                    a .brand href="/" { "Git Archiver" }
                }
                main { (content) }
            }
        }
    }
}

/// Lucide `archive`.
fn icon_archive() -> Markup {
    PreEscaped(
        r#"<svg class="icon" viewBox="0 0 24 24" aria-hidden="true"><rect width="20" height="5" x="2" y="3" rx="1"/><path d="M4 8v11a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8"/><path d="M10 12h4"/></svg>"#
            .to_owned(),
    )
}

/// Lucide `circle-check`.
fn icon_circle_check() -> Markup {
    PreEscaped(
        r#"<svg class="icon icon-circle-check" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/></svg>"#
            .to_owned(),
    )
}

/// Lucide `clock`.
fn icon_clock() -> Markup {
    PreEscaped(
        r#"<svg class="icon icon-clock" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="10"/><path d="M12 6v6l4 2"/></svg>"#
            .to_owned(),
    )
}
