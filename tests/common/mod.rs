//! Test harness: runs the whole app in-process on a random port against a
//! temporary data directory and a clock the test controls, and talks to it
//! over HTTP.

#![allow(dead_code)]

pub mod browser;
pub mod clock;
pub mod forge;
pub mod git;
pub mod list_page;
pub mod logs;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use git_archiver::{Options, RunningApp};
use tempfile::TempDir;

use clock::ManualClock;
use forge::{FakeForge, Upstream};

pub const MINUTE: Duration = Duration::from_secs(60);
pub const HOUR: Duration = Duration::from_secs(3600);

pub struct TestApp {
    dir: TempDir,
    clock: Arc<ManualClock>,
    app: Option<RunningApp>,
}

pub struct Page {
    pub status: u16,
    /// The path the response came from, after any redirect.
    pub path: String,
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

/// Three upstream repos, `a/one`, `a/two` and `a/three`, with one commit
/// each, and their repo identities.
pub fn three_repos(forge: &FakeForge) -> (Vec<Upstream>, Vec<String>) {
    let paths = ["a/one", "a/two", "a/three"];
    let repos = paths
        .iter()
        .map(|path| {
            let upstream = forge.create_repo(path);
            upstream.commit("first");
            upstream
        })
        .collect();
    let ids = paths
        .iter()
        .map(|path| format!("http://{}/{path}", forge.host()))
        .collect();
    (repos, ids)
}

/// The upstreams' URLs, for tracking them.
pub fn urls(repos: &[Upstream]) -> Vec<&str> {
    repos.iter().map(Upstream::url).collect()
}

/// Each repo's last success, in Unix seconds, in the order given.
pub async fn last_successes(app: &TestApp, ids: &[String]) -> Vec<Option<u64>> {
    let metrics = app.metrics().await;
    ids.iter().map(|id| metrics.last_success(id)).collect()
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
            let next = self.next_fetch().await;
            if self.clock.idle() && next.is_some_and(|next| next > self.now()) {
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

    /// When the scheduler's next fetch is due, in Unix seconds.
    pub async fn next_fetch(&self) -> Option<u64> {
        self.metrics().await.next_fetch()
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
        self.get_with_headers(path, &[]).await
    }

    /// Like [`TestApp::get`], but also sends these request headers, the way a
    /// reverse proxy in front of the app would.
    pub async fn get_with_headers(&self, path: &str, headers: &[(&str, &str)]) -> Page {
        let url = format!("http://{}{}", self.addr(), path);
        let mut request = reqwest::Client::new().get(&url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        Page::from(request.send().await.expect("request succeeds")).await
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

    /// The list page, parsed.
    pub async fn list_page(&self) -> list_page::ListPage {
        self.page("/").await
    }

    /// The page at `path`, parsed, after checking it was found.
    pub async fn page(&self, path: &str) -> list_page::ListPage {
        let page = self.get(path).await;
        assert_eq!(page.status, 200, "GET {path}: {}", page.text());
        list_page::ListPage::parse(&page.text())
    }

    /// Opens the untrack dialog on the card in the pane of the page at
    /// `path` and clicks the button labelled `choice`, the way a browser
    /// submits it.
    pub async fn untrack_from_page(&self, path: &str, choice: &str) -> Page {
        use list_page::{select_in, text};
        let list = self.page(path).await;
        let card = list.pane_card().expect("the page has a pane");
        let form = select_in(&card, "dialog form")
            .into_iter()
            .next()
            .expect("the pane's card has an untrack dialog");
        assert_eq!(form.value().attr("method"), Some("post"));
        let mut fields: Vec<(&str, &str)> = select_in(&form, "input[type=hidden]")
            .iter()
            .map(|input| {
                let input = input.value();
                (
                    input.attr("name").unwrap(),
                    input.attr("value").unwrap_or(""),
                )
            })
            .collect();
        let button = select_in(&form, "button")
            .into_iter()
            .find(|button| text(button) == choice)
            .unwrap_or_else(|| panic!("the untrack dialog has no {choice:?} button"));
        if let Some(name) = button.value().attr("name") {
            fields.push((name, button.value().attr("value").unwrap_or("")));
        }
        self.post_form(form.value().attr("action").unwrap(), &fields)
            .await
    }

    /// Opens the event stream at `/events`, as an open page does, and
    /// returns once the app has answered with its headers: from then on,
    /// every change reaches it.
    pub async fn events(&self) -> Events {
        let url = format!("http://{}/events", self.addr());
        let response = reqwest::Client::new()
            .get(&url)
            .header("accept", "text/event-stream")
            .send()
            .await
            .expect("request succeeds");
        assert_eq!(response.status().as_u16(), 200, "GET /events");
        Events {
            response,
            buffer: String::new(),
        }
    }

    /// Fills in the add form in the list page's add dialog with this URL and
    /// submits it.
    pub async fn add_repo(&self, url: &str) -> Page {
        let page = self.get("/").await.text();
        let action = {
            let html = scraper::Html::parse_document(&page);
            let selector = scraper::Selector::parse("dialog form:has(input[name=url])").unwrap();
            let form = html
                .select(&selector)
                .next()
                .unwrap_or_else(|| panic!("the list page has no add dialog: {page}"));
            assert_eq!(form.value().attr("method"), Some("post"));
            form.value()
                .attr("action")
                .expect("form has an action")
                .to_owned()
        };
        self.post_form(&action, &[("url", url)]).await
    }
}

/// How long [`Events::changed`] waits for a `changed` event.
const EVENT_PATIENCE: Duration = Duration::from_secs(10);

/// An open connection to `/events`.
pub struct Events {
    response: reqwest::Response,
    /// What arrived after the last complete event.
    buffer: String,
}

/// What the event stream sent: an event, or a comment such as a heartbeat.
#[derive(Debug, PartialEq, Eq)]
enum Sent {
    /// An event, by its fields as sent, e.g. `["event: changed", "data:"]`.
    Event(Vec<String>),
    Comment,
}

impl Events {
    /// A response header's value, if the app sent it.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.response
            .headers()
            .get(name)
            .map(|v| v.to_str().expect("header is text"))
    }

    /// Waits for the next `changed` event, and fails the test if none comes
    /// within [`EVENT_PATIENCE`]. The event carries nothing but its name and
    /// the empty data browsers need to see it.
    pub async fn changed(&mut self) {
        let fields = self
            .next_event_within(EVENT_PATIENCE)
            .await
            .unwrap_or_else(|| panic!("no `changed` event within {EVENT_PATIENCE:?}"));
        assert_eq!(
            fields,
            ["event: changed", "data:"],
            "a `changed` event, empty"
        );
    }

    /// Fails the test if an event arrives within `wait`.
    pub async fn stays_quiet_for(&mut self, wait: Duration) {
        if let Some(fields) = self.next_event_within(wait).await {
            panic!("unexpected event: {fields:?}");
        }
    }

    /// The fields of the next event, skipping comments, or `None` if none
    /// arrives within `wait`.
    async fn next_event_within(&mut self, wait: Duration) -> Option<Vec<String>> {
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.next_within(left).await? {
                Sent::Event(fields) => return Some(fields),
                Sent::Comment => {}
            }
        }
    }

    /// The next event or comment, or `None` if nothing complete arrives
    /// within `wait`.
    async fn next_within(&mut self, wait: Duration) -> Option<Sent> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let Some(end) = self.buffer.find("\n\n") {
                let block: String = self.buffer.drain(..end + 2).collect();
                let lines: Vec<String> = block[..end].lines().map(str::to_owned).collect();
                return Some(if lines.iter().all(|line| line.starts_with(':')) {
                    Sent::Comment
                } else {
                    Sent::Event(lines)
                });
            }
            let chunk = tokio::time::timeout_at(deadline, self.response.chunk())
                .await
                .ok()?
                .expect("the event stream stays readable")
                .expect("the event stream stays open");
            self.buffer
                .push_str(std::str::from_utf8(&chunk).expect("events are UTF-8"));
        }
    }
}

impl Page {
    async fn from(response: reqwest::Response) -> Self {
        let status = response.status().as_u16();
        let path = response.url().path().to_owned();
        let content_type = response
            .headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default();
        let body = response.bytes().await.unwrap().to_vec();
        Page {
            status,
            path,
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

    /// When the scheduler's next fetch is due, in Unix seconds.
    pub fn next_fetch(&self) -> Option<u64> {
        self.value("archiver_next_fetch_timestamp_seconds", &[])
            .map(whole_seconds)
    }

    /// When the repo was last fetched successfully, in Unix seconds.
    pub fn last_success(&self, repo: &str) -> Option<u64> {
        self.value(
            "archiver_repo_last_success_timestamp_seconds",
            &[("repo", repo)],
        )
        .map(whole_seconds)
    }
}

/// A timestamp sample as whole Unix seconds; fails on anything else, so the
/// conversion never hides a fraction.
fn whole_seconds(value: f64) -> u64 {
    assert!(
        value >= 0.0 && value.fract() == 0.0,
        "not whole Unix seconds: {value}"
    );
    value as u64
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
