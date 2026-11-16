//! The web UI: server-rendered HTML, with the stylesheet and fonts embedded in
//! the binary so the page loads nothing from other hosts.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::Router;
use axum::extract::{Form, Path, Request, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use serde::Deserialize;
use tokio::sync::Notify;

use crate::archive::{self, Archives, Inspection};
use crate::clock::Clock;
use crate::config::{self, AddError};
use crate::git_http;
use crate::identity::RepoId;
use crate::metrics;
use crate::overview::{SharedOverview, Tracked};
use crate::state::{RepoState, Status};

pub struct AppState {
    pub config_path: PathBuf,
    pub clock: Arc<dyn Clock>,
    pub archives: Arc<Archives>,
    pub overview: SharedOverview,
    pub wake_scheduler: Arc<Notify>,
}

/// How long the add form waits for upstream to answer.
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);

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
        .route("/repos", post(add_repo))
        .route("/metrics", get(metrics))
        .route("/git/{*path}", any(git))
        .route("/assets/style.css", get(stylesheet))
        .route("/assets/fonts/{name}", get(font))
        .with_state(state)
}

async fn metrics(State(state): State<Arc<AppState>>) -> Response {
    let ids: Vec<RepoId> = {
        let overview = state.overview.lock().unwrap();
        overview.tracked.iter().map(|t| t.id.clone()).collect()
    };
    let mut preserved_refs = BTreeMap::new();
    for id in ids {
        match state.archives.preserved(&id).await {
            Ok(Some(refs)) => {
                preserved_refs.insert(id.to_string(), refs.len());
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(repo = %id, error = %format!("{e:#}"), "couldn't read archive")
            }
        }
    }
    let body = metrics::render(&state.overview.lock().unwrap(), &preserved_refs);
    ([(header::CONTENT_TYPE, metrics::CONTENT_TYPE)], body).into_response()
}

async fn git(State(state): State<Arc<AppState>>, request: Request) -> Response {
    git_http::serve(state.archives.root(), request).await
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

#[derive(Deserialize)]
struct AddRepoRequest {
    #[serde(default)]
    url: String,
}

/// What the add form shows: the URL typed and why it wasn't added.
#[derive(Default)]
struct AddForm {
    url: String,
    error: Option<String>,
}

async fn add_repo(
    State(app): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
    Form(form): Form<AddRepoRequest>,
) -> Response {
    let url = form.url.trim();
    match try_add(&app, url).await {
        Ok(()) => Redirect::to("/").into_response(),
        Err(error) => {
            let form = AddForm {
                url: url.to_owned(),
                error: Some(error),
            };
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                render_list(&app, &form, &endpoint_base(&headers, &uri)).await,
            )
                .into_response()
        }
    }
}

/// Checks the URL, makes sure upstream answers, writes it into the config
/// and queues its clone. On failure, says why for the form.
async fn try_add(app: &AppState, url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("Enter the URL of the repo to add.".to_owned());
    }
    let id = config::check_repo_url(url).map_err(|problem| format!("“{url}” {problem}."))?;
    let token = config::check_add(&app.config_path, &id).map_err(|e| add_error(app, url, e))?;

    let deadline = app.clock.now() + PROBE_TIMEOUT;
    let probe = tokio::select! {
        result = archive::probe(url, token.as_ref()) => result,
        () = app.clock.sleep_until(deadline) => {
            Err("it didn’t answer within a minute.".to_owned())
        }
    };
    probe.map_err(|message| format!("Couldn’t reach “{url}”: {message}"))?;

    config::add_repo(&app.config_path, url, &id, |repos| {
        let mut overview = app.overview.lock().unwrap();
        overview.add_tracked(repos, id.clone(), app.clock.now());
    })
    .map_err(|e| add_error(app, url, e))?;
    tracing::info!(repo = %id, "added");
    app.wake_scheduler.notify_one();
    Ok(())
}

fn add_error(app: &AppState, url: &str, error: AddError) -> String {
    if let AddError::Invalid(e) | AddError::Io(e) = &error {
        tracing::error!(error = %format!("{e:#}"), "couldn't add a repo to the config");
    }
    match error {
        AddError::AlreadyTracked(existing) if existing == url => {
            format!("“{url}” is already tracked.")
        }
        AddError::AlreadyTracked(existing) => {
            format!("“{url}” is already tracked as “{existing}”.")
        }
        AddError::Invalid(_) => format!(
            "Couldn’t add the repo: the config file is invalid. Fix {} and try again.",
            app.config_path.display()
        ),
        AddError::Io(e) => format!(
            "Couldn’t add the repo: {} can’t be read or written ({}). \
             Check that the app may write to it and try again.",
            app.config_path.display(),
            e.root_cause()
        ),
    }
}

async fn list_page(State(app): State<Arc<AppState>>, headers: HeaderMap, uri: Uri) -> Html<String> {
    render_list(&app, &AddForm::default(), &endpoint_base(&headers, &uri)).await
}

async fn render_list(app: &AppState, form: &AddForm, base: &str) -> Html<String> {
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
        let archived = app.archives.exists(&repo.id);
        cards.push((repo, state, inspection, archived));
    }
    let now = app.clock.now();

    Html(
        layout(html! {
            (add_form(form))
            @if cards.is_empty() {
                section .card.empty {
                    (icon_archive())
                    h2 { "No repos tracked yet" }
                    p {
                        "Add one above, or list repo URLs under " code { "repos" } " in "
                        code { (app.config_path.display()) }
                        ". The app picks up changes to the file within a minute."
                    }
                }
            } @else {
                section aria-labelledby="tracked-heading" {
                    h2 #tracked-heading .section-label {
                        "Tracked repos " span .count { (cards.len()) }
                    }
                    ul .repos {
                        @for (repo, state, inspection, archived) in &cards {
                            li {
                                (repo_card(repo, state, inspection, archived.then_some(base), now))
                            }
                        }
                    }
                }
            }
        })
        .into_string(),
    )
}

fn add_form(form: &AddForm) -> Markup {
    let invalid = form.error.is_some();
    html! {
        section .card.add-repo {
            form method="post" action="/repos" {
                label for="repo-url" { "Repo URL" }
                div .add-row {
                    input #repo-url type="url" name="url" required
                        placeholder="https://github.com/owner/repo"
                        autocomplete="off" spellcheck="false"
                        value=(form.url)
                        aria-invalid=[invalid.then_some("true")]
                        aria-describedby=[invalid.then_some("add-error")];
                    button .button.primary type="submit" { "Add repo" }
                }
                @if let Some(error) = &form.error {
                    p #add-error .form-error role="alert" {
                        (icon_circle_alert())
                        span { (error) }
                    }
                }
            }
        }
    }
}

fn repo_card(
    repo: &Tracked,
    state: &RepoState,
    inspection: &Inspection,
    // Where the git endpoint is, if the repo has an archive to clone yet.
    endpoint_base: Option<&str>,
    now: SystemTime,
) -> Markup {
    let clone = endpoint_base.map(|base| git_http::clone_command(base, &repo.id));
    let fetched = state.status == Some(Status::Available);
    html! {
        article .card {
            h3 .repo-url { (repo.url) }
            @match (state.status, state.last_success) {
                (Some(Status::Available), Some(success)) => p .status.available {
                    (icon_circle_check())
                    span { "Fetched " (time(success, now)) }
                },
                (Some(Status::Unavailable), _) => {
                    p .status.unavailable {
                        (icon_ban())
                        span {
                            "Unavailable"
                            @if let Some(since) = state.status_since {
                                " since " (date(since))
                            }
                            @if let Some(http_status) = state.http_status {
                                " · HTTP " (http_status)
                            }
                        }
                    }
                    (reason(state))
                },
                (Some(Status::FetchFailed), _) => {
                    p .status.fetch-failed {
                        (icon_refresh_cw_off())
                        span { "Fetch failed" }
                    }
                    (reason(state))
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
                            Some(sha) => (short_sha(sha)),
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
            @if let Some(preserved) = &inspection.preserved {
                (preserved_refs(preserved))
            }
            @if let Some(clone) = clone {
            div .clone-line {
                code .clone { (clone) }
                button .icon-button type="button" data-copy=(clone)
                    aria-label="Copy clone command" title="Copy" {
                    (icon_copy())
                    (icon_check())
                }
            }
            }
        }
    }
}

/// Git's message for the last failed fetch, as git wrote it.
fn reason(state: &RepoState) -> Markup {
    html! {
        @if let Some(reason) = &state.reason {
            pre .reason { (reason) }
        }
    }
}

/// How many refs the archive preserved from before rewrites, expanding to
/// their names and SHAs.
fn preserved_refs(preserved: &BTreeMap<String, String>) -> Markup {
    let label = html! {
        (icon_history())
        span { span .count { (preserved.len()) } " preserved ref" @if preserved.len() != 1 { "s" } }
    };
    html! {
        @if preserved.is_empty() {
            p .preserved { (label) }
        } @else {
            details .preserved {
                summary { (label) }
                ul .preserved-refs {
                    @for (name, sha) in preserved {
                        li { code .ref { (name) } (short_sha(sha)) }
                    }
                }
            }
        }
    }
}

/// A `<time>` showing the UTC date, e.g. "23 Oct 2026", with the exact time
/// on hover.
fn date(at: SystemTime) -> Markup {
    let exact = humantime::format_rfc3339_seconds(at).to_string();
    let (year, month, day) = (&exact[0..4], &exact[5..7], &exact[8..10]);
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][month.parse::<usize>().expect("RFC 3339 has a month") - 1];
    let day = day.trim_start_matches('0');
    time_element(at, &format!("{day} {month} {year}"))
}

/// `http://<host>[:<port>]` as the browser reached the app, which is how
/// git clients reach the git endpoint too.
fn endpoint_base(headers: &HeaderMap, uri: &Uri) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| uri.authority().map(|a| a.as_str()))
        .unwrap_or("localhost");
    format!("http://{host}")
}

/// The first 12 characters of a SHA, with the whole SHA on hover.
fn short_sha(sha: &str) -> Markup {
    html! { code .sha title=(sha) { (sha.get(..12).unwrap_or(sha)) } }
}

/// A `<time>` showing how long ago `then` was, with the exact UTC time on
/// hover.
fn time(then: SystemTime, now: SystemTime) -> Markup {
    time_element(then, &time_ago(then, now))
}

/// A `<time>` for `at` showing `text`, with the exact UTC time on hover.
fn time_element(at: SystemTime, text: &str) -> Markup {
    let exact = humantime::format_rfc3339_seconds(at).to_string();
    html! {
        time datetime=(exact) title=(exact.replace('T', " ").replace('Z', " UTC")) {
            (text)
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
                script { (PreEscaped(COPY_SCRIPT)) }
            }
        }
    }
}

/// Copies a `data-copy` button's text. The clipboard API needs a secure
/// context, which a plain-HTTP server on another host isn't, so it falls
/// back to selecting the text and copying the selection, as it does when the
/// browser denies clipboard access.
const COPY_SCRIPT: &str = r#"
document.addEventListener("click", (event) => {
  const button = event.target.closest("[data-copy]");
  if (!button) return;
  const copied = () => {
    button.classList.add("copied");
    setTimeout(() => button.classList.remove("copied"), 1500);
  };
  const selectAndCopy = () => {
    const range = document.createRange();
    range.selectNodeContents(button.previousElementSibling);
    getSelection().removeAllRanges();
    getSelection().addRange(range);
    if (document.execCommand("copy")) copied();
  };
  if (navigator.clipboard && window.isSecureContext) {
    navigator.clipboard.writeText(button.dataset.copy).then(copied, selectAndCopy);
  } else {
    selectAndCopy();
  }
});
"#;

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

/// Lucide `ban`.
fn icon_ban() -> Markup {
    PreEscaped(
        r#"<svg class="icon icon-ban" viewBox="0 0 24 24" aria-hidden="true"><path d="M4.929 4.929 19.07 19.071"/><circle cx="12" cy="12" r="10"/></svg>"#
            .to_owned(),
    )
}

/// Lucide `refresh-cw-off`.
fn icon_refresh_cw_off() -> Markup {
    PreEscaped(
        r#"<svg class="icon icon-refresh-cw-off" viewBox="0 0 24 24" aria-hidden="true"><path d="M21 8L18.74 5.74A9.75 9.75 0 0 0 12 3C11 3 10.03 3.16 9.13 3.47"/><path d="M8 16H3v5"/><path d="M3 12C3 9.51 4 7.26 5.64 5.64"/><path d="m3 16 2.26 2.26A9.75 9.75 0 0 0 12 21c2.49 0 4.74-1 6.36-2.64"/><path d="M21 12c0 1-.16 1.97-.47 2.87"/><path d="M21 3v5h-5"/><path d="M22 22 2 2"/></svg>"#
            .to_owned(),
    )
}

/// Lucide `circle-alert`.
fn icon_circle_alert() -> Markup {
    PreEscaped(
        r#"<svg class="icon icon-circle-alert" viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="10"/><line x1="12" x2="12" y1="8" y2="12"/><line x1="12" x2="12.01" y1="16" y2="16"/></svg>"#
            .to_owned(),
    )
}

/// Lucide `history`.
fn icon_history() -> Markup {
    PreEscaped(
        r#"<svg class="icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8"/><path d="M3 3v5h5"/><path d="M12 7v5l4 2"/></svg>"#
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

/// Lucide `copy`.
fn icon_copy() -> Markup {
    PreEscaped(
        r#"<svg class="icon icon-copy" viewBox="0 0 24 24" aria-hidden="true"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>"#
            .to_owned(),
    )
}

/// Lucide `check`, shown briefly after copying.
fn icon_check() -> Markup {
    PreEscaped(
        r#"<svg class="icon icon-check" viewBox="0 0 24 24" aria-hidden="true"><path d="M20 6 9 17l-5-5"/></svg>"#
            .to_owned(),
    )
}
