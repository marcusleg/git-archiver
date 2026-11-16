//! `config.toml`: loading, validation and the default file written on first start.

use std::collections::BTreeMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use serde::Deserialize;

use crate::identity::RepoId;

const DEFAULT_CONFIG: &str = include_str!("default_config.toml");

#[derive(Debug, Clone)]
pub struct Config {
    pub interval: Duration,
    pub listen: SocketAddr,
    pub repos: Vec<String>,
    #[expect(dead_code, reason = "validated now; fetching uses the tokens")]
    pub hosts: BTreeMap<String, HostConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    pub token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default = "default_interval")]
    interval: String,
    #[serde(default = "default_listen")]
    listen: String,
    #[serde(default)]
    repos: Vec<String>,
    #[serde(default)]
    hosts: BTreeMap<String, HostConfig>,
}

fn default_interval() -> String {
    "24h".to_owned()
}

fn default_listen() -> String {
    "0.0.0.0:8080".to_owned()
}

pub fn file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("config.toml")
}

/// Writes the commented default config unless a config already exists, then
/// loads and validates it.
pub fn load_or_init(data_dir: &Path) -> anyhow::Result<Config> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("couldn't create data directory {}", data_dir.display()))?;
    let path = file_path(data_dir);
    write_default_if_missing(&path)?;
    load(&path)
}

fn write_default_if_missing(path: &Path) -> anyhow::Result<()> {
    let context = || format!("couldn't write default config {}", path.display());
    let dir = path.parent().expect("config path has a parent");
    let mut file = tempfile::NamedTempFile::new_in(dir).with_context(context)?;
    file.write_all(DEFAULT_CONFIG.as_bytes())
        .and_then(|()| file.as_file().sync_all())
        .with_context(context)?;
    // Moves the complete file into place only if no config exists, so an
    // existing config is never overwritten and a failed write never leaves a
    // partial one behind.
    match file.persist_noclobber(path) {
        Ok(_) => {
            tracing::info!(path = %path.display(), "wrote default config");
            Ok(())
        }
        Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e.error).with_context(context),
    }
}

pub fn load(path: &Path) -> anyhow::Result<Config> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("couldn't read config {}", path.display()))?;
    parse(&text).with_context(|| format!("invalid config {}", path.display()))
}

fn parse(text: &str) -> anyhow::Result<Config> {
    let raw: RawConfig = toml::from_str(text)?;
    Ok(Config {
        interval: parse_interval(&raw.interval)?,
        listen: raw.listen.parse().with_context(|| {
            format!(
                "`listen` must be an IP address and port such as \"0.0.0.0:8080\", got {:?}",
                raw.listen
            )
        })?,
        repos: raw
            .repos
            .into_iter()
            .map(validate_repo_url)
            .collect::<anyhow::Result<_>>()?,
        hosts: validate_hosts(raw.hosts)?,
    })
}

fn parse_interval(text: &str) -> anyhow::Result<Duration> {
    let interval = humantime::parse_duration(text).with_context(|| {
        format!("`interval` must be a duration such as \"24h\" or \"90m\", got {text:?}")
    })?;
    if interval.is_zero() {
        bail!("`interval` must be longer than zero, got {text:?}");
    }
    Ok(interval)
}

fn validate_repo_url(repo: String) -> anyhow::Result<String> {
    let url = url::Url::parse(&repo)
        .with_context(|| format!("`repos` entry {repo:?} is not a valid URL"))?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("`repos` entry {repo:?} must be an http:// or https:// URL");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!(
            "`repos` entry {repo:?} must not contain credentials; \
             set a token under [hosts.\"<host>\"] instead"
        );
    }
    if url.host_str().is_none_or(str::is_empty) {
        bail!("`repos` entry {repo:?} has no host");
    }
    if url.path().trim_matches('/').is_empty() {
        bail!("`repos` entry {repo:?} has no repository path");
    }
    RepoId::from_url(&repo).with_context(|| format!("`repos` entry {repo:?} can't be archived"))?;
    Ok(repo)
}

fn validate_hosts(
    hosts: BTreeMap<String, HostConfig>,
) -> anyhow::Result<BTreeMap<String, HostConfig>> {
    for (host, settings) in &hosts {
        if settings.token.trim().is_empty() {
            bail!("`hosts.\"{host}\".token` must not be empty");
        }
    }
    Ok(hosts)
}
