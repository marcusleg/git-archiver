//! The web UI: server-rendered HTML, with the stylesheet and fonts embedded in
//! the binary so the page loads nothing from other hosts.

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use maud::{DOCTYPE, Markup, PreEscaped, html};

use crate::config::Config;

pub struct AppState {
    pub config: Config,
    pub config_path: PathBuf,
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
        .route("/assets/style.css", get(stylesheet))
        .route("/assets/fonts/{name}", get(font))
        .with_state(state)
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

async fn list_page(State(state): State<Arc<AppState>>) -> Html<String> {
    let repos = &state.config.repos;
    Html(
        layout(html! {
            @if repos.is_empty() {
                section .card.empty {
                    (icon_archive())
                    h2 { "No repos tracked yet" }
                    p {
                        "Add repo URLs to " code { "repos" } " in "
                        code { (state.config_path.display()) }
                        ", then restart the app."
                    }
                }
            } @else {
                section aria-labelledby="tracked-heading" {
                    h2 #tracked-heading .section-label {
                        "Tracked repos " span .count { (repos.len()) }
                    }
                    ul .repos {
                        @for repo in repos {
                            li {
                                article .card {
                                    h3 .repo-url { (repo) }
                                }
                            }
                        }
                    }
                }
            }
        })
        .into_string(),
    )
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
