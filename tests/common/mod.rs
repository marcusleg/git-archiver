//! Test harness: runs the whole app in-process on a random port against a
//! temporary data directory, and talks to it over HTTP.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use git_archiver::{Options, RunningApp};
use tempfile::TempDir;

pub struct TestApp {
    dir: TempDir,
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

impl TestApp {
    /// Starts the app against an empty data directory.
    pub async fn start() -> Self {
        let dir = TempDir::new().unwrap();
        let mut app = Self { dir, app: None };
        app.boot().await.expect("app starts");
        app
    }

    /// Starts the app against a data directory holding the given `config.toml`.
    pub async fn with_config(config: &str) -> Self {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("config.toml"), config).unwrap();
        let mut app = Self { dir, app: None };
        app.boot().await.expect("app starts");
        app
    }

    /// Tries to start the app against a data directory holding the given
    /// `config.toml`, returning the startup error.
    pub async fn start_error(config: &str) -> String {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("config.toml"), config).unwrap();
        match git_archiver::start(options(dir.path())).await {
            Ok(_) => panic!("app started with an invalid config"),
            Err(e) => format!("{e:#}"),
        }
    }

    async fn boot(&mut self) -> anyhow::Result<()> {
        self.app = Some(git_archiver::start(options(self.dir.path())).await?);
        Ok(())
    }

    /// Stops the app and starts it again against the same data directory.
    pub async fn restart(&mut self) {
        if let Some(app) = self.app.take() {
            app.shutdown().await;
        }
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

    pub fn addr(&self) -> SocketAddr {
        self.app.as_ref().expect("app is running").addr()
    }

    pub async fn get(&self, path: &str) -> Page {
        let url = format!("http://{}{}", self.addr(), path);
        let response = reqwest::get(&url).await.expect("request succeeds");
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

/// Binds to a random local port, overriding the config's `listen`.
fn options(data_dir: &Path) -> Options {
    Options {
        data_dir: data_dir.to_owned(),
        listen: Some("127.0.0.1:0".parse().unwrap()),
    }
}
