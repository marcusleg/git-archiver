//! A fake forge: serves local bare repos over git's smart HTTP protocol by
//! running `git http-backend` as a CGI, the way a real git host would answer.
//! Tests change upstream by pushing to it.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;

use super::git::git;

pub struct FakeForge {
    dir: TempDir,
    addr: SocketAddr,
    server: JoinHandle<()>,
    behaviour: Arc<Behaviour>,
}

/// How the forge answers, shared with its request handler.
struct Behaviour {
    served: PathBuf,
    /// Repos that fail instead of being served, by request path prefix.
    failing: Mutex<HashMap<String, Failure>>,
    hung_requests: AtomicUsize,
}

/// A way the forge can refuse or fail to serve a repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// 403 with GitHub's DMCA `remote:` message.
    DmcaTakedown,
    /// 404 "Repository not found", as GitHub answers for a deleted repo.
    NotFound,
    /// 401 asking for credentials, as GitHub answers anonymous requests for
    /// some repos.
    Unauthorized,
    /// 500.
    ServerError,
    /// Never answers.
    Hang,
}

pub const DMCA_MESSAGE: &str = "Repository unavailable due to DMCA takedown.";
pub const DMCA_NOTICE: &str =
    "https://github.com/github/dmca/blob/master/2026/10/2026-10-23-example.md";

impl Drop for FakeForge {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl FakeForge {
    pub async fn start() -> Self {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("served")).unwrap();
        std::fs::create_dir(dir.path().join("work")).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let behaviour = Arc::new(Behaviour {
            served: dir.path().join("served"),
            failing: Mutex::default(),
            hung_requests: AtomicUsize::new(0),
        });
        let router = axum::Router::new()
            .fallback(http_backend)
            .with_state(behaviour.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            dir,
            addr,
            server,
            behaviour,
        }
    }

    /// Makes every request for the repo at `path` hang without an answer.
    pub fn hang(&self, path: &str) {
        self.fail(path, Failure::Hang);
    }

    /// Makes every request for the repo at `path` fail this way, until
    /// [`Self::restore`].
    pub fn fail(&self, path: &str, failure: Failure) {
        self.behaviour
            .failing
            .lock()
            .unwrap()
            .insert(format!("/{path}.git/"), failure);
    }

    /// Serves the repo at `path` normally again.
    pub fn restore(&self, path: &str) {
        self.behaviour
            .failing
            .lock()
            .unwrap()
            .remove(&format!("/{path}.git/"));
    }

    /// Stops answering altogether: connections to the forge are refused
    /// from now on.
    pub async fn go_offline(&self) {
        self.server.abort();
        while !self.server.is_finished() {
            tokio::task::yield_now().await;
        }
    }

    /// Waits (in real time) until a request has hung.
    pub async fn wait_for_hung_request(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.behaviour.hung_requests.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < deadline, "no request hung");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The host part of every repo URL, as it appears in archive paths.
    pub fn host(&self) -> String {
        self.addr.to_string()
    }

    /// Creates an empty repo at `http://<host>/<path>.git` whose default
    /// branch is `main`.
    pub fn create_repo(&self, path: &str) -> Upstream {
        self.create_repo_with_default_branch(path, "main")
    }

    pub fn create_repo_with_default_branch(&self, path: &str, branch: &str) -> Upstream {
        let bare = self.dir.path().join("served").join(format!("{path}.git"));
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "-q", "--bare", "-b", branch]);
        let work = self.dir.path().join("work").join(path);
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", branch]);
        git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        Upstream {
            url: format!("http://{}/{path}.git", self.addr),
            bare,
            work,
        }
    }
}

/// One repo on the fake forge, with a working copy to make changes in.
pub struct Upstream {
    url: String,
    /// The repo the forge serves.
    bare: PathBuf,
    work: PathBuf,
}

impl Upstream {
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The served repo on disk, for fetching from it without going through
    /// the forge (which a blocking `git` call in a test would deadlock).
    pub fn served_path(&self) -> &str {
        self.bare.to_str().unwrap()
    }

    /// Commits a change on the current branch, pushes it and returns the
    /// commit's SHA.
    pub fn commit(&self, message: &str) -> String {
        std::fs::write(self.work.join("file.txt"), message).unwrap();
        git(&self.work, &["add", "-A"]);
        git(&self.work, &["commit", "-q", "-m", message]);
        git(&self.work, &["push", "-q", "origin", "HEAD"]);
        git(&self.work, &["rev-parse", "HEAD"])
    }

    /// Tags the current commit with an annotated tag and pushes it.
    pub fn tag(&self, name: &str) {
        git(&self.work, &["tag", "-a", "-m", name, name]);
        git(
            &self.work,
            &["push", "-q", "origin", &format!("refs/tags/{name}")],
        );
    }

    /// Pushes the current commit as another branch.
    pub fn push_branch(&self, name: &str) {
        git(
            &self.work,
            &["push", "-q", "origin", &format!("HEAD:refs/heads/{name}")],
        );
    }

    /// Force-pushes a new root commit over the current branch, so its new tip
    /// doesn't descend from the old one. Returns the new tip.
    pub fn force_push_new_history(&self, message: &str) -> String {
        let branch = git(&self.work, &["symbolic-ref", "--short", "HEAD"]);
        git(&self.work, &["checkout", "-q", "--orphan", "rewritten"]);
        std::fs::write(self.work.join("file.txt"), message).unwrap();
        git(&self.work, &["add", "-A"]);
        git(&self.work, &["commit", "-q", "-m", message]);
        git(&self.work, &["branch", "-q", "-M", &branch]);
        git(&self.work, &["push", "-q", "--force", "origin", "HEAD"]);
        git(&self.work, &["rev-parse", "HEAD"])
    }

    /// Moves an existing tag to the current commit and force-pushes it.
    pub fn move_tag(&self, name: &str) {
        git(&self.work, &["tag", "-f", "-a", "-m", name, name]);
        git(
            &self.work,
            &[
                "push",
                "-q",
                "--force",
                "origin",
                &format!("refs/tags/{name}"),
            ],
        );
    }

    /// Makes a new branch at the current commit upstream's default branch,
    /// the way a forge's "rename default branch" setting would. The old
    /// branch stays upstream.
    pub fn switch_default_branch(&self, name: &str) {
        git(&self.work, &["checkout", "-q", "-b", name]);
        git(&self.work, &["push", "-q", "origin", "HEAD"]);
        git(
            &self.bare,
            &["symbolic-ref", "HEAD", &format!("refs/heads/{name}")],
        );
    }

    pub fn delete_tag(&self, name: &str) {
        git(
            &self.work,
            &["push", "-q", "origin", &format!(":refs/tags/{name}")],
        );
    }
}

/// Answers one request by running `git http-backend` as a CGI script.
async fn http_backend(State(behaviour): State<Arc<Behaviour>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let failure = {
        let failing = behaviour.failing.lock().unwrap();
        failing
            .iter()
            .find(|(prefix, _)| parts.uri.path().starts_with(prefix.as_str()))
            .map(|(_, failure)| *failure)
    };
    if let Some(failure) = failure {
        return fail(&behaviour, failure).await;
    }
    let body = to_bytes(body, usize::MAX).await.unwrap();

    let mut command = tokio::process::Command::new("git");
    command
        .arg("http-backend")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_PROJECT_ROOT", &behaviour.served)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("REQUEST_METHOD", parts.method.as_str())
        .env("PATH_INFO", parts.uri.path())
        .env("QUERY_STRING", parts.uri.query().unwrap_or(""))
        .env("CONTENT_LENGTH", body.len().to_string())
        .env("REMOTE_ADDR", "127.0.0.1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (name, value) in &parts.headers {
        let value = value.to_str().unwrap_or_default();
        match name.as_str() {
            "content-type" => command.env("CONTENT_TYPE", value),
            "git-protocol" => command.env("GIT_PROTOCOL", value),
            _ => command.env(cgi_header_var(name.as_str()), value),
        };
    }

    let mut child = command.spawn().expect("git http-backend starts");
    let mut stdin = child.stdin.take().unwrap();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&body).await;
    });
    let output = child.wait_with_output().await.unwrap();
    let _ = writer.await;
    cgi_response(&output.stdout)
}

async fn fail(behaviour: &Behaviour, failure: Failure) -> Response {
    let (status, body) = match failure {
        Failure::DmcaTakedown => (
            StatusCode::FORBIDDEN,
            format!("{DMCA_MESSAGE}\nSee the takedown notice for more details:\n  {DMCA_NOTICE}\n"),
        ),
        Failure::NotFound => (StatusCode::NOT_FOUND, "Repository not found.\n".to_owned()),
        Failure::Unauthorized => (StatusCode::UNAUTHORIZED, String::new()),
        Failure::ServerError => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Internal server error\n".to_owned(),
        ),
        Failure::Hang => {
            behaviour.hung_requests.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<()>().await;
            unreachable!()
        }
    };
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert("content-type", HeaderValue::from_static("text/plain"));
    if failure == Failure::Unauthorized {
        headers.insert(
            "www-authenticate",
            HeaderValue::from_static("Basic realm=\"GitHub\""),
        );
    }
    response
}

fn cgi_header_var(name: &str) -> String {
    format!("HTTP_{}", name.to_ascii_uppercase().replace('-', "_"))
}

fn cgi_response(output: &[u8]) -> Response {
    let split = output
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, i + 4))
        .or_else(|| {
            output
                .windows(2)
                .position(|w| w == b"\n\n")
                .map(|i| (i, i + 2))
        })
        .expect("CGI output has a header block");
    let head = std::str::from_utf8(&output[..split.0]).unwrap();
    let mut response = Response::new(Body::from(output[split.1..].to_vec()));
    for line in head.lines() {
        let (name, value) = line.split_once(':').unwrap();
        let value = value.trim();
        if name.eq_ignore_ascii_case("status") {
            let code = value.split_whitespace().next().unwrap().parse().unwrap();
            *response.status_mut() = StatusCode::from_u16(code).unwrap();
        } else {
            response.headers_mut().append(
                HeaderName::try_from(name).unwrap(),
                HeaderValue::try_from(value).unwrap(),
            );
        }
    }
    response
}
