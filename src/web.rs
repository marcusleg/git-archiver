//! The web UI: server-rendered HTML, with the stylesheet and fonts embedded in
//! the binary so the page loads nothing from other hosts.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use axum::Router;
use axum::extract::{Form, Path, Request, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{any, get, post};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use serde::Deserialize;

use crate::archive::{Archives, Inspection};
use crate::clock::Clock;
use crate::git_http;
use crate::identity::RepoId;
use crate::metrics;
use crate::overview::{RepoView, SharedOverview};
use crate::state::{RepoState, Status};
use crate::tracking::{ArchiveChoice, TrackOutcome, Tracking, UntrackOutcome};

pub struct AppState {
    pub config_path: PathBuf,
    pub clock: Arc<dyn Clock>,
    pub archives: Arc<Archives>,
    pub overview: SharedOverview,
    pub tracking: Arc<Tracking>,
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
        .route("/repos", post(add_repo))
        .route("/repos/untrack", post(untrack_repo))
        .route("/metrics", get(metrics))
        .route("/git/{*path}", any(git))
        .route("/assets/style.css", get(stylesheet))
        .route("/assets/fonts/{name}", get(font))
        .with_state(state)
}

async fn metrics(State(state): State<Arc<AppState>>) -> Response {
    let on_disk = state.archives.list().await;
    let view = state.overview.view(&on_disk);
    let mut preserved_refs = BTreeMap::new();
    for repo in &view.repos {
        let id = &repo.id;
        match state.archives.preserved(id).await {
            Ok(Some(refs)) => {
                preserved_refs.insert(id.to_string(), refs.len());
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(repo = %id, error = %format!("{e:#}"), "couldn't read archive")
            }
        }
    }
    let body = metrics::render(&view, &preserved_refs);
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
                render_list(&app, &form, None, &endpoint_base(&headers, &uri)).await,
            )
                .into_response()
        }
    }
}

/// Asks Tracking to track the URL. On failure, says why for the form.
async fn try_add(app: &AppState, url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("Enter the URL of the repo to add.".to_owned());
    }
    match app.tracking.track(url).await {
        TrackOutcome::Tracked => Ok(()),
        TrackOutcome::AlreadyTracked(existing) if existing == url => {
            Err(format!("“{existing}” is already tracked."))
        }
        TrackOutcome::AlreadyTracked(existing) => {
            Err(format!("“{url}” is already tracked as “{existing}”."))
        }
        TrackOutcome::Invalid(problem) => Err(format!("“{url}” {problem}.")),
        TrackOutcome::CouldNotBeChecked(message) => {
            Err(format!("Couldn’t reach “{url}”: {message}"))
        }
        TrackOutcome::ConfigInvalid => Err(config_invalid(app, "add the repo")),
        TrackOutcome::ConfigUnwritable(cause) => {
            Err(config_unwritable(app, "add the repo", &cause))
        }
    }
}

/// `action` is what the person tried, such as "add the repo".
fn config_invalid(app: &AppState, action: &str) -> String {
    format!(
        "Couldn’t {action}: the config file is invalid. Fix {} and try again.",
        app.config_path.display()
    )
}

fn config_unwritable(app: &AppState, action: &str, cause: &str) -> String {
    format!(
        "Couldn’t {action}: {} can’t be read or written ({cause}). \
         Check that the app may write to it and try again.",
        app.config_path.display(),
    )
}

#[derive(Deserialize)]
struct UntrackRequest {
    /// The repo's identity.
    repo: String,
    archive: ArchiveChoice,
}

async fn untrack_repo(
    State(app): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
    Form(form): Form<UntrackRequest>,
) -> Response {
    let Ok(id) = RepoId::from_url(&form.repo) else {
        return (StatusCode::BAD_REQUEST, "Not a repo identity.").into_response();
    };
    match try_untrack(&app, &id, form.archive).await {
        Ok(()) => Redirect::to("/").into_response(),
        Err(error) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            render_list(
                &app,
                &AddForm::default(),
                Some(&error),
                &endpoint_base(&headers, &uri),
            )
            .await,
        )
            .into_response(),
    }
}

/// Untracks the repo through `Tracking`. On failure, says why for the page.
async fn try_untrack(app: &AppState, id: &RepoId, archive: ArchiveChoice) -> Result<(), String> {
    match app.tracking.untrack(id, archive).await {
        // NotTracked: already untracked, e.g. a form sent twice. Deleting an
        // untracked archive isn't offered.
        UntrackOutcome::Untracked | UntrackOutcome::NotTracked => Ok(()),
        UntrackOutcome::ArchiveNotDeleted(cause) => Err(format!(
            "Untracked “{}”, but couldn’t delete its archive: {cause}. \
             Check that the app may write to {} and delete it by hand.",
            id.host_and_path(),
            app.archives.path(id).display()
        )),
        UntrackOutcome::ConfigInvalid => Err(config_invalid(app, "untrack the repo")),
        UntrackOutcome::ConfigUnwritable(cause) => {
            Err(config_unwritable(app, "untrack the repo", &cause))
        }
    }
}

async fn list_page(State(app): State<Arc<AppState>>, headers: HeaderMap, uri: Uri) -> Html<String> {
    render_list(
        &app,
        &AddForm::default(),
        None,
        &endpoint_base(&headers, &uri),
    )
    .await
}

/// A card's contents: one archive, tracked or not.
struct CardView {
    /// The URL as configured for a tracked repo, the identity otherwise.
    title: String,
    id: RepoId,
    state: RepoState,
    inspection: Inspection,
    archived: bool,
    tracked: bool,
}

async fn card_view(app: &AppState, repo: &RepoView) -> CardView {
    let id = repo.id.clone();
    let inspection = app.archives.inspect(&id).await.unwrap_or_else(|e| {
        tracing::warn!(repo = %id, error = %format!("{e:#}"), "couldn't read archive");
        Inspection::default()
    });
    let archived = app.archives.exists(&id);
    CardView {
        title: repo.title.clone(),
        id,
        state: repo.state.clone().unwrap_or_default(),
        inspection,
        archived,
        tracked: repo.tracked,
    }
}

/// The list page. `problem` says why the last action failed.
async fn render_list(
    app: &AppState,
    form: &AddForm,
    problem: Option<&str>,
    base: &str,
) -> Html<String> {
    let on_disk = app.archives.list().await;
    let view = app.overview.view(&on_disk);
    let config_error = view.config_error;
    let (tracked, untracked): (Vec<_>, Vec<_>) = view.repos.into_iter().partition(|r| r.tracked);
    // The pane shows the first tracked repo; untracked archives get no
    // pane of their own until rows can be selected.
    let selected = match tracked.first() {
        Some(first) => Some(card_view(app, first).await),
        None => None,
    };
    let selected_id = selected.as_ref().map(|card| &card.id);
    let now = app.clock.now();

    Html(
        layout(html! {
            @if let Some(problem) = problem {
                div .card.banner role="alert" {
                    (icon_circle_alert())
                    p { (problem) }
                }
            }
            @if let Some(error) = &config_error {
                div .card.banner role="status" {
                    (icon_circle_alert())
                    div {
                        p {
                            code { (app.config_path.display()) }
                            " is invalid, so the app keeps using the last good config. \
                             Fix the file and the app picks it up within a minute."
                        }
                        pre .reason { (error) }
                    }
                }
            }
            div .columns {
                div .list {
                    @if tracked.is_empty() {
                        section .card.empty {
                            (icon_archive())
                            h2 { "No repos tracked yet" }
                            p {
                                "Add one here, or list repo URLs under " code { "repos" } " in "
                                code { (app.config_path.display()) }
                                ". The app picks up changes to the file within a minute."
                            }
                            (add_button())
                        }
                    } @else {
                        section aria-labelledby="tracked-heading" {
                            div .section-head {
                                h2 #tracked-heading .section-label {
                                    "Tracked repos " span .count { (tracked.len()) }
                                }
                                (add_button())
                            }
                            (repo_table(&tracked, selected_id, now))
                        }
                    }
                    @if !untracked.is_empty() {
                        section aria-labelledby="untracked-heading" {
                            h2 #untracked-heading .section-label {
                                "Untracked archives " span .count { (untracked.len()) }
                            }
                            p .section-note {
                                "No longer fetched. Add a repo again to continue its archive."
                            }
                            (repo_table(&untracked, None, now))
                        }
                    }
                }
                @if let Some(card) = &selected {
                    aside .pane aria-label="Selected repo" {
                        (repo_card(card, base, now))
                    }
                }
            }
            (add_dialog(form))
        })
        .into_string(),
    )
}

/// A table of repos, one row each, marking the row of the repo the pane
/// shows.
fn repo_table(repos: &[RepoView], selected: Option<&RepoId>, now: SystemTime) -> Markup {
    html! {
        div .card.table-card {
            table .repo-table {
                thead {
                    tr {
                        th scope="col" { "Repo" }
                        th scope="col" { "Status" }
                        th scope="col" { "Last success" }
                        th .num scope="col" { "Size" }
                    }
                }
                tbody {
                    @for repo in repos {
                        (repo_row(repo, selected == Some(&repo.id), now))
                    }
                }
            }
        }
    }
}

/// A repo's row: its identity with the configured URL on hover, its
/// status, last success and size. `selected` marks the pane's repo.
fn repo_row(repo: &RepoView, selected: bool, now: SystemTime) -> Markup {
    let state = repo.state.clone().unwrap_or_default();
    html! {
        tr aria-current=[selected.then_some("true")] {
            td .repo title=(repo.title) {
                span .host { (repo.id.host()) "/" }
                strong { (repo.id.path()) }
            }
            td { (status_label(&state)) }
            td { (time_or_never(state.last_success, now)) }
            td .num {
                (size_or_dash(state.size_bytes))
            }
        }
    }
}

/// A repo's status as an icon and a short label, for its row.
fn status_label(state: &RepoState) -> Markup {
    match state.status {
        Some(Status::Available) => html! {
            span .status.available { (icon_circle_check()) span { "Available" } }
        },
        Some(Status::Unavailable) => html! {
            span .status.unavailable {
                (icon_ban())
                span {
                    "Unavailable"
                    @if let Some(http_status) = state.http_status { " · " (http_status) }
                }
            }
        },
        Some(Status::FetchFailed) => html! {
            span .status.fetch-failed { (icon_refresh_cw_off()) span { "Fetch failed" } }
        },
        None => html! {
            span .status { (icon_clock()) span { "Not fetched yet" } }
        },
    }
}

/// The untrack button and its confirmation dialog.
fn untrack_dialog(id: &RepoId) -> Markup {
    let dialog = "untrack";
    html! {
        button .button.secondary.small type="button" commandfor=(dialog)
            command="show-modal" aria-haspopup="dialog" { "Untrack" }
        dialog #(dialog) .dialog aria-labelledby={ (dialog) "-title" }
            aria-describedby={ (dialog) "-body" } {
            form method="post" action="/repos/untrack" {
                input type="hidden" name="repo" value=(id);
                h2 #{ (dialog) "-title" } { "Untrack “" (id.host_and_path()) "”?" }
                p #{ (dialog) "-body" } {
                    "The app stops fetching it. You can keep the archive or delete it \
                     from disk; deleting can’t be undone."
                }
                div .dialog-actions {
                    button .button.secondary type="submit" formmethod="dialog" { "Cancel" }
                    button .button.secondary type="submit" name="archive" value="keep" {
                        "Keep archive"
                    }
                    button .button.danger type="submit" name="archive" value="delete" {
                        "Delete archive"
                    }
                }
            }
        }
    }
}

const ADD_DIALOG: &str = "add-repo";

/// The "+ Add repo" button. Invoker commands open the dialog without
/// script; the page's script covers browsers that lack them.
fn add_button() -> Markup {
    html! {
        button .button.primary.small.add-button type="button" commandfor=(ADD_DIALOG)
            command="show-modal" aria-haspopup="dialog" {
            (icon_plus())
            span { "Add repo" }
        }
    }
}

/// The add dialog. After a failed add it's open on load, with the URL as
/// typed and the reason; without script it shows in place rather than as a
/// modal.
fn add_dialog(form: &AddForm) -> Markup {
    let invalid = form.error.is_some();
    html! {
        dialog #(ADD_DIALOG) .dialog open[invalid] aria-labelledby={ (ADD_DIALOG) "-title" }
            aria-describedby={ (ADD_DIALOG) "-body" } {
            form .add-form method="post" action="/repos" {
                h2 #{ (ADD_DIALOG) "-title" } { "Add a repo" }
                p #{ (ADD_DIALOG) "-body" } {
                    "The app clones it right away, then fetches it once per interval."
                }
                label for="repo-url" { "Repo URL" }
                input #repo-url type="url" name="url" required autofocus
                    placeholder="https://github.com/owner/repo"
                    autocomplete="off" spellcheck="false"
                    value=(form.url)
                    aria-invalid=[invalid.then_some("true")]
                    aria-describedby=[invalid.then_some(format!("{ADD_DIALOG}-error"))];
                @if let Some(error) = &form.error {
                    p #{ (ADD_DIALOG) "-error" } .form-error role="alert" {
                        (icon_circle_alert())
                        span { (error) }
                    }
                }
                div .dialog-actions {
                    button .button.secondary type="submit" formmethod="dialog" formnovalidate {
                        "Cancel"
                    }
                    button .button.primary type="submit" { "Add repo" }
                }
            }
        }
    }
}

/// A repo's card. A tracked repo's card offers to untrack it; an untracked
/// archive's card is read-only.
fn repo_card(card: &CardView, base: &str, now: SystemTime) -> Markup {
    let CardView {
        title,
        id,
        state,
        inspection,
        archived,
        tracked,
    } = card;
    // Only an archive on disk can be cloned.
    let clone = archived.then(|| git_http::clone_command(base, id));
    let mirror_clone = archived.then(|| git_http::mirror_clone_command(base, id));
    let fetched = state.status == Some(Status::Available);
    // The status line, and whether the facts add "Last success": an
    // Available repo's status line already says when, and otherwise there
    // is no success to show.
    let (status_line, show_last_success) = match (state.status, state.last_success) {
        (Some(Status::Available), Some(success)) => (
            html! {
                p .status.available {
                    (icon_circle_check())
                    span { "Fetched " (time(success, now)) }
                }
            },
            false,
        ),
        (Some(Status::Unavailable), _) => (
            html! {
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
            true,
        ),
        (Some(Status::FetchFailed), _) => (
            html! {
                p .status.fetch-failed {
                    (icon_refresh_cw_off())
                    span { "Fetch failed" }
                }
                (reason(state))
            },
            true,
        ),
        _ => (
            html! {
                p .status {
                    (icon_clock())
                    span { "Not fetched yet" }
                }
            },
            false,
        ),
    };
    html! {
        article .card {
            div .card-head {
                h3 .repo-url { (title) }
                @if *tracked {
                    (untrack_dialog(id))
                }
            }
            (status_line)
            dl .facts {
                @if show_last_success {
                    div {
                        dt { "Last success" }
                        dd { (time_or_never(state.last_success, now)) }
                    }
                }
                div {
                    dt { "Last attempt" }
                    dd { (time_or_never(state.last_attempt, now)) }
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
                        (size_or_dash(state.size_bytes))
                    }
                }
            }
            @if let Some(preserved) = &inspection.preserved {
                (preserved_refs(preserved, mirror_clone.as_deref()))
            }
            @if let Some(clone) = clone {
                (clone_line(&clone, CloneKind::Plain))
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

/// The kind of git command a clone line offers.
#[derive(Clone, Copy)]
enum CloneKind {
    /// `git clone`, for the archive's default branch and tags.
    Plain,
    /// `git clone --mirror`, which also brings the preserved refs.
    Mirror,
}

impl CloneKind {
    /// The class of the line's `code`, and the label of its copy button.
    fn class_and_label(self) -> (&'static str, &'static str) {
        match self {
            CloneKind::Plain => ("clone", "Copy clone command"),
            CloneKind::Mirror => ("mirror-clone", "Copy mirror clone command"),
        }
    }
}

/// A ready-to-copy git `command` of the given kind, with a copy button.
fn clone_line(command: &str, kind: CloneKind) -> Markup {
    let (class, label) = kind.class_and_label();
    html! {
        div .clone-line {
            code class=(class) { (command) }
            button .icon-button type="button" data-copy=(command)
                aria-label=(label) title="Copy" {
                (icon_copy())
                (icon_check())
            }
        }
    }
}

/// How many refs the archive preserved from before rewrites, expanding to
/// their names and SHAs and, when the archive can be cloned, the
/// `mirror_clone` command that brings them along.
fn preserved_refs(preserved: &BTreeMap<String, String>, mirror_clone: Option<&str>) -> Markup {
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
                @if let Some(mirror_clone) = mirror_clone {
                    p .mirror-note { "A mirror clone includes the preserved refs:" }
                    (clone_line(mirror_clone, CloneKind::Mirror))
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
    time_element(at, &format!("{day} {month} {year}"), false)
}

/// `<scheme>://<host>[:<port>]` as the browser reached the app, which is how
/// git clients reach the git endpoint too. The host comes from `Host`, the
/// scheme from [`forwarded_scheme`].
fn endpoint_base(headers: &HeaderMap, uri: &Uri) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| uri.authority().map(|a| a.as_str()))
        .unwrap_or("localhost");
    format!("{}://{host}", forwarded_scheme(headers))
}

const X_FORWARDED_PROTO: header::HeaderName = header::HeaderName::from_static("x-forwarded-proto");

/// The scheme the client reached a reverse proxy in front of the app on:
/// the `proto` of the first element of `Forwarded` (RFC 7239), else the first
/// value of `X-Forwarded-Proto`, else `http`. Only `http` and `https` count.
///
/// These headers are trusted from anyone, without a list of trusted proxies,
/// on purpose: the scheme only goes into the clone commands shown back to the
/// same client, so a client that spoofs them only misleads itself.
fn forwarded_scheme(headers: &HeaderMap) -> &'static str {
    let header_str = |name| headers.get(name).and_then(|v| v.to_str().ok());
    let forwarded = header_str(header::FORWARDED).and_then(|value| {
        first_forwarded_element(value)
            .split(';')
            .filter_map(|pair| pair.split_once('='))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("proto"))
            .and_then(|(_, proto)| known_scheme(unquote(proto.trim())))
    });
    forwarded
        .or_else(|| {
            header_str(X_FORWARDED_PROTO)
                .and_then(|value| known_scheme(value.split(',').next()?.trim()))
        })
        .unwrap_or("http")
}

/// The first element of a `Forwarded` value, the hop nearest the client: up
/// to the first comma outside a quoted string.
fn first_forwarded_element(value: &str) -> &str {
    let mut quoted = false;
    for (i, c) in value.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => return &value[..i],
            _ => {}
        }
    }
    value
}

/// `value` without the double quotes around it, if it has them.
fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

/// `http` or `https` for `scheme` in any case, or `None` for anything else.
fn known_scheme(scheme: &str) -> Option<&'static str> {
    ["http", "https"]
        .into_iter()
        .find(|known| scheme.eq_ignore_ascii_case(known))
}

/// The first 12 characters of a SHA, with the whole SHA on hover.
fn short_sha(sha: &str) -> Markup {
    html! { code .sha title=(sha) { (sha.get(..12).unwrap_or(sha)) } }
}

/// A `<time>` showing how long ago `then` was, with the exact UTC time on
/// hover. `data-ago` lets the page's script count it up while the page is
/// open.
fn time(then: SystemTime, now: SystemTime) -> Markup {
    time_element(then, &time_ago(then, now), true)
}

/// [`time`] for `then`, or "Never" without one.
fn time_or_never(then: Option<SystemTime>, now: SystemTime) -> Markup {
    match then {
        Some(then) => time(then, now),
        None => html! { "Never" },
    }
}

/// A `<time>` for `at` showing `text`, with the exact UTC time on hover,
/// marked `data-ago` if `text` says how long ago `at` was.
fn time_element(at: SystemTime, text: &str, ago: bool) -> Markup {
    let exact = humantime::format_rfc3339_seconds(at).to_string();
    html! {
        time datetime=(exact) title=(exact.replace('T', " ").replace('Z', " UTC")) data-ago[ago] {
            (text)
        }
    }
}

/// How long ago `then` was, in words. The page's script ([`SCRIPT`]) words it
/// the same way when it counts the time up, so keep the two in step.
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

/// [`format_size`] for `bytes`, or a muted dash without them.
fn size_or_dash(bytes: Option<u64>) -> Markup {
    match bytes {
        Some(bytes) => html! { (format_size(bytes)) },
        None => html! { span .muted { "—" } },
    }
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
                script { (PreEscaped(SCRIPT)) }
            }
        }
    }
}

/// Opens the dialog a `commandfor` button names where the browser lacks
/// invoker commands, shows a dialog the page opened on load as a modal,
/// copies a `data-copy` button's text, and once a minute rewords every
/// `data-ago` time the way [`time_ago`] does, by the browser's clock.
///
/// The clipboard API needs a secure context, which a plain-HTTP server on
/// another host isn't, so copying falls back to selecting the text and
/// copying the selection, as it does when the browser denies clipboard
/// access.
const SCRIPT: &str = r#"
for (const dialog of document.querySelectorAll("dialog[open]")) {
  dialog.close();
  dialog.showModal();
}
document.addEventListener("click", (event) => {
  const invoker = event.target.closest("[commandfor]");
  if (invoker && !("commandForElement" in invoker)) {
    const dialog = document.getElementById(invoker.getAttribute("commandfor"));
    if (!dialog.open) dialog.showModal();
  }
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

const timeAgo = (then) => {
  const secs = Math.floor((Date.now() - then) / 1000);
  if (secs < 60) return "just now";
  const [count, unit] =
    secs < 3600 ? [Math.floor(secs / 60), "minute"]
    : secs < 86400 ? [Math.floor(secs / 3600), "hour"]
    : [Math.floor(secs / 86400), "day"];
  return `${count} ${unit}${count === 1 ? "" : "s"} ago`;
};
setInterval(() => {
  for (const time of document.querySelectorAll("time[data-ago]")) {
    time.textContent = timeAgo(Date.parse(time.dateTime));
  }
}, 60000);
"#;

/// Lucide `plus`.
fn icon_plus() -> Markup {
    PreEscaped(
        r#"<svg class="icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M5 12h14"/><path d="M12 5v14"/></svg>"#
            .to_owned(),
    )
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
