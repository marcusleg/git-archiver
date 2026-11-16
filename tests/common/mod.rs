//! Test harness: runs the whole app in-process on a random port against a
//! temporary data directory and a clock the test controls, and talks to it
//! over HTTP.

#![allow(dead_code)]

pub mod clock;
pub mod forge;
pub mod git;
pub mod logs;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use git_archiver::{Options, RunningApp};
use tempfile::TempDir;

use clock::ManualClock;

pub const HOUR: Duration = Duration::from_secs(3600);

pub struct TestApp {
    dir: TempDir,
    clock: Arc<ManualClock>,
    app: Option<RunningApp>,
}

pub struct Page {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

impl Page {
    pub fn text(&self) -> String {
        String::from_utf8(self.body.clone()).expect("response body is UTF-8")
    }
}

/// A config tracking the given repo URLs, with every other setting at its
/// default.
pub fn config_tracking(urls: &[&str]) -> String {
    let repos: Vec<String> = urls.iter().map(|url| format!("  {url:?},\n")).collect();
    format!("repos = [\n{}]\n", repos.concat())
}

impl TestApp {
    /// Starts the app against an empty data directory.
    pub async fn start() -> Self {
        let app = Self::new();
        app.launch().await
    }

    /// Starts the app against a data directory holding the given `config.toml`.
    pub async fn with_config(config: &str) -> Self {
        let app = Self::new();
        app.write_config(config);
        app.launch().await
    }

    /// Starts the app tracking the given repo URLs.
    pub async fn tracking(urls: &[&str]) -> Self {
        Self::with_config(&config_tracking(urls)).await
    }

    /// Tries to start the app against a data directory holding the given
    /// `config.toml`, returning the startup error.
    pub async fn start_error(config: &str) -> String {
        let app = Self::new();
        app.write_config(config);
        match git_archiver::start(app.options()).await {
            Ok(_) => panic!("app started with an invalid config"),
            Err(e) => format!("{e:#}"),
        }
    }

    fn new() -> Self {
        Self {
            dir: TempDir::new().unwrap(),
            clock: ManualClock::new(),
            app: None,
        }
    }

    async fn launch(mut self) -> Self {
        self.boot().await.expect("app starts");
        self
    }

    async fn boot(&mut self) -> anyhow::Result<()> {
        self.app = Some(git_archiver::start(self.options()).await?);
        Ok(())
    }

    /// Binds to a random local port, overriding the config's `listen`.
    fn options(&self) -> Options {
        Options {
            data_dir: self.dir.path().to_owned(),
            listen: Some("127.0.0.1:0".parse().unwrap()),
            clock: self.clock.clone(),
        }
    }

    /// Stops the app and starts it again against the same data directory.
    /// The clock keeps its time.
    pub async fn restart(&mut self) {
        self.stop().await;
        self.boot().await.expect("app restarts");
    }

    pub async fn stop(&mut self) {
        if let Some(app) = self.app.take() {
            app.shutdown().await;
        }
    }

    /// Starts a stopped app again against the same data directory.
    pub async fn start_again(&mut self) {
        self.boot().await.expect("app restarts");
    }

    pub fn data_dir(&self) -> &Path {
        self.dir.path()
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.path().join("config.toml")
    }

    pub fn config(&self) -> String {
        std::fs::read_to_string(self.config_path()).unwrap()
    }

    pub fn write_config(&self, config: &str) {
        std::fs::write(self.config_path(), config).unwrap();
    }

    /// The archive of a repo, by host and path: `repos/<host>/<path>.git`.
    pub fn archive(&self, host_and_path: &str) -> PathBuf {
        self.dir
            .path()
            .join("repos")
            .join(format!("{host_and_path}.git"))
    }

    pub fn addr(&self) -> SocketAddr {
        self.app.as_ref().expect("app is running").addr()
    }

    /// The test clock's current time, in Unix seconds.
    pub fn now(&self) -> u64 {
        use git_archiver::clock::Clock;
        self.clock
            .now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// Moves the clock forward without waiting for the app to react.
    pub fn advance(&self, by: Duration) {
        self.clock.advance(by);
    }

    /// Moves the clock forward, then waits until every fetch due by then has
    /// run.
    pub async fn advance_and_settle(&self, by: Duration) {
        self.advance(by);
        self.settle().await;
    }

    /// Waits until the scheduler has run everything due at the current time:
    /// it waits on the clock for nothing that is already due, and its next
    /// fetch lies in the future (so it isn't stuck in a fetch either).
    pub async fn settle(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let next = self
                .metrics()
                .await
                .value("archiver_next_fetch_timestamp_seconds", &[]);
            if self.clock.idle() && next.is_some_and(|next| next > self.now() as f64) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "scheduler didn't settle; next fetch at {next:?}, now {}",
                self.now()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub async fn metrics(&self) -> Metrics {
        let page = self.get("/metrics").await;
        assert_eq!(page.status, 200);
        Metrics::parse(&page.text())
    }

    /// Sends a GET with the request target exactly as given, unnormalized,
    /// and returns the status code.
    pub async fn raw_get_status(&self, target: &str) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut stream = tokio::net::TcpStream::connect(self.addr()).await.unwrap();
        let request = format!(
            "GET {target} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            self.addr()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        let response = String::from_utf8_lossy(&response);
        let status_line = response.lines().next().expect("a status line");
        status_line
            .split_whitespace()
            .nth(1)
            .and_then(|code| code.parse().ok())
            .unwrap_or_else(|| panic!("bad status line: {status_line}"))
    }

    pub async fn get(&self, path: &str) -> Page {
        let url = format!("http://{}{}", self.addr(), path);
        Page::from(reqwest::get(&url).await.expect("request succeeds")).await
    }

    /// Submits a form the way a browser does, following the redirect after a
    /// successful post.
    pub async fn post_form(&self, path: &str, fields: &[(&str, &str)]) -> Page {
        let url = format!("http://{}{}", self.addr(), path);
        let response = reqwest::Client::new()
            .post(&url)
            .form(fields)
            .send()
            .await
            .expect("request succeeds");
        Page::from(response).await
    }

    /// Fills in the list page's add form with this URL and submits it.
    pub async fn add_repo(&self, url: &str) -> Page {
        let page = self.get("/").await.text();
        let action = {
            let html = scraper::Html::parse_document(&page);
            let selector = scraper::Selector::parse("form:has(input[name=url])").unwrap();
            let form = html
                .select(&selector)
                .next()
                .unwrap_or_else(|| panic!("the list page has no add form: {page}"));
            assert_eq!(form.value().attr("method"), Some("post"));
            form.value()
                .attr("action")
                .expect("form has an action")
                .to_owned()
        };
        self.post_form(&action, &[("url", url)]).await
    }
}

impl Page {
    async fn from(response: reqwest::Response) -> Self {
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let body = response.bytes().await.unwrap().to_vec();
        Page {
            status,
            content_type,
            body,
        }
    }
}

/// Samples from a Prometheus text exposition.
pub struct Metrics {
    pub text: String,
    samples: Vec<Sample>,
}

pub struct Sample {
    pub name: String,
    pub labels: Vec<(String, String)>,
    pub value: f64,
}

impl Metrics {
    pub fn parse(text: &str) -> Self {
        let samples = text
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(parse_sample)
            .collect();
        Self {
            text: text.to_owned(),
            samples,
        }
    }

    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    /// The value of the sample with exactly these labels.
    pub fn value(&self, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
        self.samples
            .iter()
            .find(|s| {
                s.name == name
                    && s.labels.len() == labels.len()
                    && labels
                        .iter()
                        .all(|(k, v)| s.labels.iter().any(|(sk, sv)| sk == k && sv == v))
            })
            .map(|s| s.value)
    }

    /// The repo's status: the one `archiver_repo_status` sample that is 1.
    /// Checks there is a sample for every status and the others are 0.
    pub fn repo_status(&self, repo: &str) -> Option<&'static str> {
        let statuses = ["available", "unavailable", "fetch_failed"];
        let values: Vec<Option<f64>> = statuses
            .iter()
            .map(|status| {
                self.value(
                    "archiver_repo_status",
                    &[("repo", repo), ("status", status)],
                )
            })
            .collect();
        if values.iter().all(Option::is_none) {
            return None;
        }
        let ones: Vec<&'static str> = statuses
            .iter()
            .zip(&values)
            .filter(|(_, value)| **value == Some(1.0))
            .map(|(status, _)| *status)
            .collect();
        assert!(
            ones.len() == 1
                && values
                    .iter()
                    .all(|v| matches!(v, Some(v) if *v == 0.0 || *v == 1.0)),
            "one status at 1, the others at 0: {values:?}"
        );
        Some(ones[0])
    }

    /// `archiver_fetch_errors_total` for a status.
    pub fn fetch_errors(&self, status: &str) -> Option<f64> {
        self.value("archiver_fetch_errors_total", &[("status", status)])
    }

    /// When the repo was last fetched successfully, in Unix seconds.
    pub fn last_success(&self, repo: &str) -> Option<u64> {
        self.value(
            "archiver_repo_last_success_timestamp_seconds",
            &[("repo", repo)],
        )
        .map(|v| v as u64)
    }
}

/// Parses `name{label="value",...} value`, unescaping label values.
fn parse_sample(line: &str) -> Sample {
    let (series, value) = line.rsplit_once(' ').expect("sample has a value");
    let value = value
        .parse()
        .unwrap_or_else(|_| panic!("bad value: {line}"));
    let Some((name, rest)) = series.split_once('{') else {
        return Sample {
            name: series.to_owned(),
            labels: Vec::new(),
            value,
        };
    };
    let mut labels = Vec::new();
    let mut chars = rest.strip_suffix('}').expect("labels close").chars();
    loop {
        let key: String = chars.by_ref().take_while(|c| *c != '=').collect();
        if key.is_empty() {
            break;
        }
        assert_eq!(chars.next(), Some('"'), "label value is quoted: {line}");
        let mut value = String::new();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => value.push('\n'),
                    Some(c) => value.push(c),
                    None => panic!("dangling escape: {line}"),
                },
                c => value.push(c),
            }
        }
        labels.push((key, value));
        match chars.next() {
            Some(',') | None => {}
            Some(c) => panic!("unexpected {c:?} after label: {line}"),
        }
    }
    Sample {
        name: name.to_owned(),
        labels,
        value,
    }
}
