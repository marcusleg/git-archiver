//! The git endpoint: serves every archive under `repos/` read-only over git's
//! smart HTTP protocol at `/git/<host>/<path>.git`, by running
//! `git http-backend` as a CGI.
//!
//! Serving needs no lock against the scheduler's fetches. A served clone only
//! reads, and a fetch writes its objects before it moves any ref, moves refs
//! in one atomic transaction and never prunes objects (auto-gc is off), so a
//! clone sees either the old refs or the new ones, with all their objects.
//! The staging refs a fetch in progress holds are hidden from clients.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::Context;
use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::io::ReaderStream;

use crate::archive;
use crate::identity::RepoId;

/// Where the endpoint lives in the app's URL space.
pub const PREFIX: &str = "/git/";

/// Upload-pack requests carry the client's wants and haves; even a large
/// negotiation stays far below this.
const MAX_REQUEST_BODY: usize = 64 * 1024 * 1024;

/// The smart HTTP requests the endpoint passes to `git http-backend`: the
/// ref advertisement and the pack request for fetching, and the same for
/// pushing so that git refuses them with its own message. Dumb HTTP isn't
/// served.
const SERVICES: &[&str] = &["git-upload-pack", "git-receive-pack"];

/// The command that clones a repo's archive from the endpoint, for a server
/// reached at `base` (`http://<host>[:<port>]`).
pub fn clone_command(base: &str, id: &RepoId) -> String {
    format!("git clone {}", endpoint_url(base, id))
}

/// Like [`clone_command`], but a mirror clone, which also brings along the
/// preserved refs a plain clone leaves out.
pub fn mirror_clone_command(base: &str, id: &RepoId) -> String {
    format!("git clone --mirror {}", endpoint_url(base, id))
}

/// A repo's archive's URL on the endpoint.
fn endpoint_url(base: &str, id: &RepoId) -> String {
    format!("{base}{PREFIX}{}.git", id.host_and_path())
}

/// Answers a request under `/git/` from the archives in `repos`.
pub async fn serve(repos: &Path, request: Request) -> Response {
    let path = request.uri().path();
    let query = request.uri().query().unwrap_or("");
    let Some(target) = resolve(repos, path, query) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match run_backend(target, request).await {
        Ok(response) => response,
        Err(e) => {
            tracing::error!(error = %format!("{e:#}"), "couldn't serve git request");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// A request that names an existing archive and a smart HTTP service, as
/// `git http-backend` takes it.
#[derive(Debug)]
struct BackendRequest {
    /// The canonical `repos/` directory.
    root: PathBuf,
    /// `/<host>/<path>.git/<service path>`, relative to `root`.
    path_info: String,
}

/// Maps a request path to an archive inside `repos`, or `None` (a 404) when
/// it names anything else. The path stays percent-encoded, as the archive
/// directories' names are, so an encoded `/` is part of a name, never a
/// separator. Dot segments are refused, encoded or not.
fn resolve(repos: &Path, path: &str, query: &str) -> Option<BackendRequest> {
    let segments: Vec<&str> = path.strip_prefix(PREFIX)?.split('/').collect();
    if !segments.iter().all(|s| is_plain_segment(s)) {
        return None;
    }
    // `<host>/<path...>.git`: at least a host and one path segment. Only the
    // last path segment of a repo identity may end in `.git`.
    let end = segments.iter().position(|s| s.ends_with(".git"))?;
    if end == 0 {
        return None;
    }
    let (archive, service) = segments.split_at(end + 1);
    let service = service.join("/");
    let allowed = match service.as_str() {
        "info/refs" => SERVICES
            .iter()
            .any(|s| query.split('&').any(|q| q == format!("service={s}"))),
        _ => SERVICES.contains(&service.as_str()),
    };
    if !allowed {
        return None;
    }

    let archive = archive.join("/");
    let root = repos.canonicalize().ok()?;
    // Canonicalizing resolves symlinks; anything that isn't exactly where
    // the path says, inside `repos/`, isn't an archive.
    let dir = root.join(&archive).canonicalize().ok()?;
    if dir != root.join(&archive) || !is_bare_repo(&dir) {
        return None;
    }
    Some(BackendRequest {
        root,
        path_info: format!("/{archive}/{service}"),
    })
}

/// A path segment that is neither empty nor a dot segment, even one
/// written with percent-encoded dots.
fn is_plain_segment(segment: &str) -> bool {
    let decoded_dots = segment.to_ascii_lowercase().replace("%2e", ".");
    !matches!(decoded_dots.as_str(), "" | "." | "..")
}

fn is_bare_repo(dir: &Path) -> bool {
    dir.join("HEAD").is_file() && dir.join("objects").is_dir() && dir.join("refs").is_dir()
}

/// Runs `git http-backend` for one request and streams its answer back.
async fn run_backend(target: BackendRequest, request: Request) -> anyhow::Result<Response> {
    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, MAX_REQUEST_BODY).await {
        Ok(body) => body,
        Err(_) => return Ok(StatusCode::PAYLOAD_TOO_LARGE.into_response()),
    };

    // A hidden ref matches by whole path components, so no trailing slash.
    let hide_staging = format!(
        "transfer.hideRefs={}",
        archive::STAGING.trim_end_matches('/')
    );
    let mut command = archive::command(
        &target.root,
        &[
            "-c",
            "http.receivepack=false",
            "-c",
            &hide_staging,
            "http-backend",
        ],
    );
    command
        .env("GIT_PROJECT_ROOT", &target.root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", &target.path_info)
        .env("REQUEST_METHOD", parts.method.as_str())
        .env("QUERY_STRING", parts.uri.query().unwrap_or(""))
        .env("CONTENT_LENGTH", body.len().to_string())
        .env_remove("REMOTE_USER")
        .stdin(Stdio::piped());
    for (header, var) in [
        ("content-type", "CONTENT_TYPE"),
        ("content-encoding", "HTTP_CONTENT_ENCODING"),
        ("git-protocol", "GIT_PROTOCOL"),
    ] {
        if let Some(value) = parts.headers.get(header).and_then(|v| v.to_str().ok()) {
            command.env(var, value);
        }
    }

    let mut child = command.spawn().context("couldn't run git http-backend")?;
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    tokio::spawn(async move {
        let _ = stdin.write_all(&body).await;
    });
    // Reaps the backend once it's done, logging what it complained about.
    // If the client goes away, dropping the body closes stdout and the
    // backend stops on the broken pipe.
    let path_info = target.path_info.clone();
    tokio::spawn(async move {
        if let Ok(output) = child.wait_with_output().await
            && !output.status.success()
        {
            tracing::warn!(
                path = path_info,
                error = %String::from_utf8_lossy(&output.stderr).trim(),
                "git http-backend failed",
            );
        }
    });

    let mut response = Response::new(Body::empty());
    loop {
        let mut line = String::new();
        let read = stdout
            .read_line(&mut line)
            .await
            .context("couldn't read git http-backend's headers")?;
        anyhow::ensure!(read > 0, "git http-backend ended before its headers");
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        let (name, value) = line
            .split_once(':')
            .with_context(|| format!("bad CGI header line {line:?}"))?;
        let value = value.trim();
        if name.eq_ignore_ascii_case("status") {
            let code = value.split_whitespace().next().unwrap_or_default();
            *response.status_mut() = StatusCode::from_bytes(code.as_bytes())
                .with_context(|| format!("bad CGI status {value:?}"))?;
        } else {
            response.headers_mut().append(
                HeaderName::try_from(name).context("bad CGI header name")?,
                HeaderValue::try_from(value).context("bad CGI header value")?,
            );
        }
    }
    *response.body_mut() = Body::from_stream(ReaderStream::new(stdout));
    Ok(response)
}
