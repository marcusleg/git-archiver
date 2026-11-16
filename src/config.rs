//! `config.toml`: loading, validation and the default file written on first start.

use std::collections::BTreeMap;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, bail};
use serde::Deserialize;
use toml_edit::{Array, DocumentMut, RawString, Value};

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
    if let Err(problem) = check_repo_url(&repo) {
        bail!("`repos` entry {repo:?} {problem}");
    }
    Ok(repo)
}

/// Why the config would reject this repo URL, as the rest of a sentence
/// that starts with the URL.
pub fn check_repo_url(repo: &str) -> Result<(), &'static str> {
    let url = url::Url::parse(repo).map_err(|_| "is not a valid URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("must be an http:// or https:// URL");
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("must not contain credentials; set a token under [hosts.\"<host>\"] instead");
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("has no host");
    }
    if url.path().trim_matches('/').is_empty() {
        return Err("has no repository path");
    }
    RepoId::from_url(repo).map_err(|_| "has an unsupported repository path")?;
    Ok(())
}

/// Why a repo couldn't be added to the config.
#[derive(Debug)]
pub enum AddError {
    /// The file doesn't hold a valid config right now. Its own error is for
    /// the log only: it may quote a token.
    Invalid(anyhow::Error),
    /// A repo with the same identity is tracked under this URL.
    AlreadyTracked(String),
    /// Reading or writing the file failed.
    Io(anyhow::Error),
}

/// Serializes changes to the config, so concurrent adds can't lose one
/// another.
static WRITE: Mutex<()> = Mutex::new(());

/// Checks that `url` could be added now: the config is valid and doesn't
/// track the same repo yet. `url` must pass [`check_repo_url`].
pub fn check_add(path: &Path, url: &str) -> Result<(), AddError> {
    let text = read(path)?;
    let config = parse(&text).map_err(AddError::Invalid)?;
    match tracked_as(&config.repos, url) {
        Some(existing) => Err(AddError::AlreadyTracked(existing.to_owned())),
        None => Ok(()),
    }
}

/// Appends `url` to `repos` and returns the new repo list. The file is edited
/// in place, so the operator's comments and formatting survive, and replaced
/// in one step, so a crash never leaves a partial config. `url` must pass
/// [`check_repo_url`].
pub fn add_repo(path: &Path, url: &str) -> Result<Vec<String>, AddError> {
    let _write = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    // A symlinked config is replaced at its target, keeping the link.
    let path = std::fs::canonicalize(path)
        .with_context(|| format!("couldn't read config {}", path.display()))
        .map_err(AddError::Io)?;
    let text = read(&path)?;
    let config = parse(&text).map_err(AddError::Invalid)?;
    if let Some(existing) = tracked_as(&config.repos, url) {
        return Err(AddError::AlreadyTracked(existing.to_owned()));
    }

    let edited = append_repo(&text, url).map_err(AddError::Invalid)?;
    let repos = parse(&edited)
        .context("the edited config doesn't parse")
        .map_err(AddError::Invalid)?
        .repos;
    let mut expected = config.repos;
    expected.push(url.to_owned());
    if repos != expected {
        return Err(AddError::Invalid(anyhow::anyhow!(
            "the edited config lists {repos:?}, expected {expected:?}"
        )));
    }
    write_atomically(&path, &edited).map_err(AddError::Io)?;
    Ok(repos)
}

fn read(path: &Path) -> Result<String, AddError> {
    std::fs::read_to_string(path)
        .with_context(|| format!("couldn't read config {}", path.display()))
        .map_err(AddError::Io)
}

/// The tracked URL with the same repo identity as `url`, if any.
fn tracked_as<'a>(repos: &'a [String], url: &str) -> Option<&'a str> {
    let id = RepoId::from_url(url).ok()?;
    repos
        .iter()
        .find(|repo| RepoId::from_url(repo).is_ok_and(|other| other == id))
        .map(String::as_str)
}

/// The config text with `url` appended to `repos`, laid out like the entries
/// around it.
fn append_repo(text: &str, url: &str) -> anyhow::Result<String> {
    let mut doc: DocumentMut = text.parse()?;
    match doc.get_mut("repos") {
        Some(repos) => {
            let repos = repos.as_array_mut().context("`repos` is not an array")?;
            append_formatted(repos, url);
        }
        None => {
            doc.insert("repos", toml_edit::value(Array::from_iter([url])));
        }
    }
    Ok(doc.to_string())
}

/// Appends `url` after the array's last entry and whatever comments follow
/// it. A multi-line array gets the new entry on its own line, indented like
/// the last entry and followed by a comma.
fn append_formatted(array: &mut Array, url: &str) {
    let raw = |raw: Option<&RawString>| raw.and_then(RawString::as_str).unwrap_or("").to_owned();
    let trailing = raw(Some(array.trailing()));
    let trailing_comma = array.trailing_comma();
    let mut indent = "  ".to_owned();
    let mut first = true;
    // Everything between the last entry (or its comma) and the closing
    // bracket.
    let mut after = trailing;
    if let Some(last) = array.iter_mut().last() {
        first = false;
        let decor = last.decor_mut();
        if let Some((_, last_line)) = raw(decor.prefix()).rsplit_once('\n') {
            indent = last_line.to_owned();
        }
        if !trailing_comma {
            after = raw(decor.suffix()) + &after;
            decor.set_suffix("");
        }
    }

    let mut value = Value::from(url);
    match after.rsplit_once('\n') {
        Some((head, closing_indent)) => {
            value.decor_mut().set_prefix(format!("{head}\n{indent}"));
            array.set_trailing(format!("\n{closing_indent}"));
            array.set_trailing_comma(true);
        }
        None => {
            value.decor_mut().set_prefix(if first { "" } else { " " });
            array.set_trailing(after);
        }
    }
    value.decor_mut().set_suffix("");
    array.push_formatted(value);
}

fn write_atomically(path: &Path, text: &str) -> anyhow::Result<()> {
    let context = || format!("couldn't write config {}", path.display());
    let dir = path.parent().expect("config path has a parent");
    let mut file = tempfile::NamedTempFile::new_in(dir).with_context(context)?;
    let permissions = std::fs::metadata(path).with_context(context)?.permissions();
    file.write_all(text.as_bytes())
        .and_then(|()| file.as_file().set_permissions(permissions))
        .and_then(|()| file.as_file().sync_all())
        .with_context(context)?;
    file.persist(path)
        .map_err(|e| e.error)
        .with_context(context)?;
    Ok(())
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
