//! `config.toml`: loading, validation and the default file written on first start.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::net::{Ipv6Addr, SocketAddr};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use base64::Engine;
use serde::Deserialize;
use toml::Spanned;
use toml::de::DeTable;
use toml_edit::{Array, DocumentMut, RawString, Value};

use crate::identity::RepoId;

const DEFAULT_CONFIG: &str = include_str!("default_config.toml");

#[derive(Debug, Clone)]
pub struct Config {
    pub interval: Duration,
    pub listen: SocketAddr,
    pub repos: Vec<String>,
    pub tokens: Tokens,
}

/// A host's settings. Each field's name is in [`APP_NAMES`] too.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostConfig {
    token: Spanned<Token>,
}

/// An access token for one host. Its `Debug` output hides it, so it can't
/// end up in a log by accident.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Token(String);

impl Token {
    /// The token itself, to hand to git. Nothing else should need it.
    pub fn reveal(&self) -> &str {
        &self.0
    }

    /// The value of an HTTP `Authorization` header sending the token as the
    /// basic auth password, under the user name GitHub expects for tokens
    /// (GitLab and Gitea accept any).
    pub fn basic_auth(&self) -> String {
        let credentials = format!("x-access-token:{}", self.0);
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(credentials)
        )
    }

    /// Empty, or holding whitespace or control characters, which no real
    /// token has and which would break git's credential protocol.
    fn is_malformed(&self) -> bool {
        self.0.is_empty() || self.0.chars().any(|c| c.is_whitespace() || c.is_control())
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<hidden>)")
    }
}

/// The configured tokens, by lowercased host (with the port, if any).
#[derive(Debug, Clone)]
pub struct Tokens(BTreeMap<String, Token>);

impl Tokens {
    /// The token for the repo's host, if one is configured.
    pub fn for_repo(&self, id: &RepoId) -> Option<&Token> {
        self.0.get(id.host())
    }
}

/// The config as written, with where each value the app checks itself sits,
/// so that its error can say where. Each field's name is in [`APP_NAMES`] too.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default = "default_interval")]
    interval: Spanned<String>,
    #[serde(default = "default_listen")]
    listen: Spanned<String>,
    #[serde(default)]
    repos: Vec<Spanned<String>>,
    #[serde(default)]
    hosts: BTreeMap<Spanned<String>, HostConfig>,
}

fn default_interval() -> Spanned<String> {
    Spanned::new(0..0, "24h".to_owned())
}

fn default_listen() -> Spanned<String> {
    Spanned::new(0..0, "0.0.0.0:8080".to_owned())
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

/// The config a file's text holds. Every config error is shown from here.
/// It goes to the startup error, the log and the list page, which anyone
/// can read, and anything the user wrote may be a credential, so the error
/// is built from text the app knows, plus only what of the file the app has
/// checked: hosts as [`shown_host`] names them and URLs as [`shown_url`]
/// names them. See [`ConfigError`].
fn parse(text: &str) -> anyhow::Result<Config> {
    check(text).map_err(|error| anyhow::anyhow!(error.shown(text)))
}

/// The config a file's text holds, or the first error in it.
fn check(text: &str) -> Result<Config, ConfigError> {
    let doc = DeTable::parse(text).map_err(|e| ConfigError::while_parsing(&e))?;
    let raw = RawConfig::deserialize(toml::de::Deserializer::from(doc.clone()))
        .map_err(|e| ConfigError::after_parsing(doc.get_ref(), &e))?;
    Ok(Config {
        interval: check_interval(&raw.interval)?,
        listen: raw.listen.get_ref().parse().map_err(|_| {
            ConfigError::at(
                &raw.listen,
                "`listen` must be an IP address and port such as \"0.0.0.0:8080\"",
            )
        })?,
        repos: raw
            .repos
            .into_iter()
            .enumerate()
            .map(|(i, repo)| check_repo_entry(i + 1, repo))
            .collect::<Result<_, _>>()?,
        tokens: check_hosts(raw.hosts)?,
    })
}

/// A config error, as only [`parse`] shows it: the line and column it starts
/// at, the key it's about if that's known and may be shown, and the problem.
struct ConfigError {
    span: Option<Range<usize>>,
    /// As [`shown_key`] shows it.
    key: Option<String>,
    /// Text the app wrote, or toml's or serde's with [`hide_file_text`]
    /// applied.
    problem: String,
}

impl ConfigError {
    /// A problem the app found with the value at `value`.
    fn at<T>(value: &Spanned<T>, problem: impl Into<String>) -> Self {
        Self {
            span: Some(value.span()),
            key: None,
            problem: problem.into(),
        }
    }

    /// A syntax error, found before the file parsed. It names no key, as
    /// there is no parsed file to take one from, and a line that looks like
    /// it sets one may well be inside a string.
    fn while_parsing(error: &toml::de::Error) -> Self {
        Self {
            span: error.span(),
            key: None,
            problem: hide_file_text(error.message(), None),
        }
    }

    /// An error serde found in the parsed file `doc`, such as an unknown
    /// field or a value of the wrong type, naming the key it's at.
    fn after_parsing(doc: &DeTable<'_>, error: &toml::de::Error) -> Self {
        let path = error.span().and_then(|span| key_at(doc, span.start));
        Self {
            span: error.span(),
            key: path.as_deref().and_then(shown_key),
            problem: hide_file_text(
                error.message(),
                path.as_deref().and_then(<[_]>::last).copied(),
            ),
        }
    }

    /// The error as the app shows it, such as
    /// ``line 3 (`hosts."github.com".…`), column 1: unknown field `…`, expected `token` ``,
    /// with the line and column counted in `text`, the file it was found in.
    fn shown(&self, text: &str) -> String {
        let Some(span) = &self.span else {
            return self.problem.clone();
        };
        let start = span.start.min(text.len());
        let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
        let line = text[..start].matches('\n').count() + 1;
        let column = text[line_start..start].chars().count() + 1;
        let problem = &self.problem;
        match &self.key {
            Some(key) => format!("line {line} (`{key}`), column {column}: {problem}"),
            None => format!("line {line}, column {column}: {problem}"),
        }
    }
}

/// The keys leading to the deepest key or value in `table` that holds byte
/// `at`, if any does.
fn key_at<'a>(table: &'a DeTable<'_>, at: usize) -> Option<Vec<&'a str>> {
    for (key, value) in table {
        let name: &str = key.get_ref();
        if key.span().contains(&at) {
            return Some(vec![name]);
        }
        // A table's span may cover only its header, so its keys are
        // searched whatever its span.
        if let Some(mut inner) = value.get_ref().as_table().and_then(|t| key_at(t, at)) {
            inner.insert(0, name);
            return Some(inner);
        }
        if value.span().contains(&at) {
            return Some(vec![name]);
        }
    }
    None
}

/// The names the app gives its settings, which an error may always show:
/// the fields of [`RawConfig`] and [`HostConfig`].
const APP_NAMES: [&str; 5] = ["interval", "listen", "repos", "hosts", "token"];

/// What toml's syntax errors quote in backticks, none of it from the file:
/// exactly the literals toml_parser 1.1.5 can say it expected (its
/// `Expected::Literal`s), which are punctuation, escape letters, number
/// prefixes and the words `true`, `false`, `inf` and `nan`. A newline it
/// names in words, outside backticks.
const TOML_TOKENS: [&str; 30] = [
    "#", "'", ",", "-", "=", "[", "]", "]]", "{", "}", "_", "\\", "\"", "\"\"\"", "'''", "0b",
    "0o", "0x", "b", "e", "f", "n", "r", "u", "U", "x", "true", "false", "inf", "nan",
];

/// A key as an error may show it, such as `hosts."github.com".…`: each part
/// that is one of the app's own names, or a `hosts` key that [`shown_host`]
/// names, and `…` for any other part, which may be a credential. None if no
/// part may be shown.
fn shown_key(path: &[&str]) -> Option<String> {
    let mut any_shown = false;
    let parts: Vec<String> = path
        .iter()
        .enumerate()
        .map(|(i, &part)| {
            // A `hosts` key is a host, whatever it's called.
            let shown = if i == 1 && path[0] == "hosts" {
                shown_host(part)
            } else {
                APP_NAMES.contains(&part).then(|| part.to_owned())
            };
            any_shown |= shown.is_some();
            shown.unwrap_or_else(|| "…".to_owned())
        })
        .collect();
    any_shown.then(|| parts.join("."))
}

/// toml's or serde's problem text with every piece it quotes from the file
/// shown as `…`: each double-quoted string, which serde writes with Rust's
/// escapes, and each backtick-quoted piece other than [`APP_NAMES`] and
/// [`TOML_TOKENS`]. serde quotes an unknown field in backticks without
/// escaping it, so `last_key`, the last key leading to where the error is,
/// is hidden first if it holds a backtick, which would end it early. Any
/// other key is hidden or shown like every other piece, so an error may
/// quote an app name that is also its key, as in ``missing field `token` ``
/// for `[hosts."token"]`.
fn hide_file_text(message: &str, last_key: Option<&str>) -> String {
    let message = match last_key {
        Some(key) if key.contains('`') => message.replacen(&format!("`{key}`"), "`…`", 1),
        _ => message.to_owned(),
    };
    let mut shown = String::new();
    let mut rest = message.trim();
    while let Some(open) = rest.find(['"', '`']) {
        shown.push_str(&rest[..open]);
        let quote = &rest[open..=open];
        let quoted = &rest[open + 1..];
        let end = if quote == "\"" {
            string_end(quoted)
        } else {
            quoted.find('`')
        };
        let piece = &quoted[..end.unwrap_or(quoted.len())];
        if quote == "`" && (APP_NAMES.contains(&piece) || TOML_TOKENS.contains(&piece)) {
            shown.push_str(&format!("`{piece}`"));
        } else {
            shown.push_str(&format!("{quote}…{quote}"));
        }
        rest = end.map_or("", |end| &quoted[end + 1..]);
    }
    shown.push_str(rest);
    shown
}

/// Where the string serde wrote ends: at the first `"` no `\` escapes.
/// None if it never ends.
fn string_end(quoted: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in quoted.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return Some(i),
            _ => {}
        }
    }
    None
}

/// The `interval` setting as a duration, if it's a valid one.
fn check_interval(text: &Spanned<String>) -> Result<Duration, ConfigError> {
    let interval = humantime::parse_duration(text.get_ref()).map_err(|_| {
        ConfigError::at(
            text,
            "`interval` must be a duration such as \"24h\" or \"90m\"",
        )
    })?;
    if interval.is_zero() {
        return Err(ConfigError::at(text, "`interval` must be longer than zero"));
    }
    Ok(interval)
}

/// The `repos` entry at `position`, counting from 1, if the config would
/// accept it.
fn check_repo_entry(position: usize, repo: Spanned<String>) -> Result<String, ConfigError> {
    if let Err(problem) = check_repo_url(repo.get_ref()) {
        let name =
            shown_url(repo.get_ref()).map_or(position.to_string(), |name| format!("{name:?}"));
        return Err(ConfigError::at(
            &repo,
            format!("`repos` entry {name} {problem}"),
        ));
    }
    Ok(repo.into_inner())
}

/// How an error names a `repos` entry or `hosts` key it rejects: from the
/// URL the text parses as, with `…` for its user info and for a scheme the
/// app doesn't support, and without its query or fragment, such as
/// `https://…@github.com/foo/bar`. None if the text doesn't parse as a URL
/// with a host, or if an `@` follows the host, as it does when a `#`, `/` or
/// `?` in a password ends the user info early and leaves the rest of it in
/// the host and path.
fn shown_url(text: &str) -> Option<String> {
    let url = url::Url::parse(text).ok()?;
    let host = url.host_str().filter(|host| !host.is_empty())?;
    if url[url::Position::BeforePath..].contains('@') {
        return None;
    }
    let scheme = match url.scheme() {
        scheme @ ("http" | "https") => scheme,
        _ => "…",
    };
    let user_info = if url.username().is_empty() && url.password().is_none() {
        ""
    } else {
        "…@"
    };
    let port = url
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let path = url.path().trim_end_matches('/');
    Some(format!("{scheme}://{user_info}{host}{port}{path}"))
}

/// The repo's identity if the config would accept this URL; otherwise why
/// not, as the rest of a sentence that starts with the URL.
pub fn check_repo_url(repo: &str) -> Result<RepoId, &'static str> {
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
    RepoId::from_url(repo).map_err(|_| "has an unsupported repository path")
}

/// Why a repo couldn't be added to the config.
#[derive(Debug)]
pub enum AddError {
    /// The file doesn't hold a valid config right now. Its own error is for
    /// the log only.
    Invalid(anyhow::Error),
    /// A repo with the same archive is tracked under this URL.
    AlreadyTracked(String),
    /// Reading or writing the file failed.
    Io(anyhow::Error),
}

/// Held while the config is read for its repo list or changed, so an add and
/// a reload never interleave.
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

/// Loads the config and hands the result to `f`, with no add in between.
pub fn load_locked<T>(path: &Path, f: impl FnOnce(anyhow::Result<Config>) -> T) -> T {
    let _lock = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    f(load(path))
}

/// Checks that the repo could be added now: the config is valid and doesn't
/// track a repo with the same archive yet. Returns the token to check
/// upstream with, if the repo's host has one.
pub fn check_add(path: &Path, id: &RepoId) -> Result<Option<Token>, AddError> {
    let (_, config) = read_for_add(path, id)?;
    Ok(config.tokens.for_repo(id).cloned())
}

/// Appends `url`, whose identity is `id`, to `repos`, then calls `on_added`
/// with the new repo list before any other add or reload can read the file.
/// The file is edited in place, so the operator's comments and formatting
/// survive, and replaced in one step, so a crash never leaves a partial
/// config.
pub fn add_repo(
    path: &Path,
    url: &str,
    id: &RepoId,
    on_added: impl FnOnce(&[String]),
) -> Result<(), AddError> {
    let _lock = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // A symlinked config is replaced at its target, keeping the link.
    let path = std::fs::canonicalize(path)
        .with_context(|| format!("couldn't read config {}", path.display()))
        .map_err(AddError::Io)?;
    let (text, config) = read_for_add(&path, id)?;

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
    on_added(&repos);
    Ok(())
}

/// Why a repo couldn't be removed from the config.
#[derive(Debug)]
pub enum RemoveError {
    /// The file doesn't hold a valid config right now. Its own error is for
    /// the log only.
    Invalid(anyhow::Error),
    /// Reading or writing the file failed.
    Io(anyhow::Error),
}

/// Removes every `repos` entry that shares the archive of `id`, then calls
/// `on_read` with the resulting repo list before any other change or reload
/// can read the file. Returns whether the config listed the repo; if it
/// didn't, the file is left alone. Like adding, the file is edited in place
/// and replaced in one step.
pub fn remove_repo(
    path: &Path,
    id: &RepoId,
    on_read: impl FnOnce(&[String]),
) -> Result<bool, RemoveError> {
    let _lock = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = std::fs::canonicalize(path)
        .with_context(|| format!("couldn't read config {}", path.display()))
        .map_err(RemoveError::Io)?;
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("couldn't read config {}", path.display()))
        .map_err(RemoveError::Io)?;
    let config = parse(&text).map_err(RemoveError::Invalid)?;
    let same_archive =
        |repo: &str| RepoId::from_url(repo).is_ok_and(|other| other.same_archive(id));
    let expected: Vec<String> = config
        .repos
        .iter()
        .filter(|repo| !same_archive(repo))
        .cloned()
        .collect();
    if expected.len() == config.repos.len() {
        on_read(&config.repos);
        return Ok(false);
    }

    let edited = remove_entries(&text, same_archive).map_err(RemoveError::Invalid)?;
    let repos = parse(&edited)
        .context("the edited config doesn't parse")
        .map_err(RemoveError::Invalid)?
        .repos;
    if repos != expected {
        return Err(RemoveError::Invalid(anyhow::anyhow!(
            "the edited config lists {repos:?}, expected {expected:?}"
        )));
    }
    write_atomically(&path, &edited).map_err(RemoveError::Io)?;
    on_read(&repos);
    Ok(true)
}

/// The config text without the `repos` entries `remove` picks, keeping the
/// file's line endings.
fn remove_entries(text: &str, remove: impl Fn(&str) -> bool) -> anyhow::Result<String> {
    keeping_line_endings(text, |text| {
        let mut doc: DocumentMut = text.parse()?;
        let repos = doc
            .get_mut("repos")
            .and_then(|repos| repos.as_array_mut())
            .context("`repos` is not an array")?;
        for i in (0..repos.len()).rev() {
            if repos.get(i).and_then(Value::as_str).is_some_and(&remove) {
                remove_formatted(repos, i);
            }
        }
        Ok(doc.to_string())
    })
}

/// Runs `edit` on the text with `\n` line endings, and gives the result the
/// text's own line endings back.
fn keeping_line_endings(
    text: &str,
    edit: impl FnOnce(&str) -> anyhow::Result<String>,
) -> anyhow::Result<String> {
    if !text.contains("\r\n") {
        return edit(text);
    }
    Ok(edit(&text.replace("\r\n", "\n"))?.replace('\n', "\r\n"))
}

/// Removes the array's entry at `index` the way a person would: an entry on
/// its own line goes with that line, including a comment after it, while
/// comment lines above it stay. An entry sharing its line with others goes
/// alone.
fn remove_formatted(array: &mut Array, index: usize) {
    let text_of =
        |raw: Option<&RawString>| raw.and_then(RawString::as_str).unwrap_or("").to_owned();
    let is_last = index + 1 == array.len();
    let removed = array.remove(index);
    let prefix = text_of(removed.decor().prefix());
    // What follows the entry (and its comma) up to the next entry or the
    // closing bracket.
    let after = if is_last {
        // Without a trailing comma, the entry's suffix runs up to the
        // closing bracket too.
        let suffix = if array.trailing_comma() {
            String::new()
        } else {
            text_of(removed.decor().suffix())
        };
        suffix + &text_of(Some(array.trailing()))
    } else {
        text_of(array.get(index).and_then(|next| next.decor().prefix()))
    };
    let joined = match (prefix.rfind('\n'), after.find('\n')) {
        // On its own line: keep what comes before that line and after it.
        (Some(line_start), Some(line_end)) => {
            format!("{}{}", &prefix[..=line_start], &after[line_end + 1..])
        }
        // Sharing its line: the space before it goes too.
        _ if is_last => after,
        _ => format!("{prefix}{}", after.trim_start_matches([' ', '\t'])),
    };
    if is_last {
        if array.is_empty() {
            array.set_trailing_comma(false);
        }
        let only_whitespace = joined.trim().is_empty();
        array.set_trailing(if array.is_empty() && only_whitespace {
            ""
        } else {
            &joined
        });
    } else if let Some(next) = array.get_mut(index) {
        next.decor_mut().set_prefix(joined);
    }
}

/// The config's text and contents, if a repo with identity `id` could be
/// added to it.
fn read_for_add(path: &Path, id: &RepoId) -> Result<(String, Config), AddError> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("couldn't read config {}", path.display()))
        .map_err(AddError::Io)?;
    let config = parse(&text).map_err(AddError::Invalid)?;
    // URLs differing only in scheme share an archive, so they count as the
    // same repo here.
    let tracked = config
        .repos
        .iter()
        .find(|repo| RepoId::from_url(repo).is_ok_and(|other| other.same_archive(id)));
    match tracked {
        Some(existing) => Err(AddError::AlreadyTracked(existing.clone())),
        None => Ok((text, config)),
    }
}

/// The config text with `url` appended to `repos`, laid out like the entries
/// around it and keeping the file's line endings.
fn append_repo(text: &str, url: &str) -> anyhow::Result<String> {
    keeping_line_endings(text, |text| {
        let mut doc: DocumentMut = text.parse()?;
        let has_settings = doc.iter().any(|(_, item)| item.is_value());
        Ok(match doc.get_mut("repos") {
            Some(repos) => {
                let repos = repos.as_array_mut().context("`repos` is not an array")?;
                append_formatted(repos, url);
                doc.to_string()
            }
            None if has_settings => {
                doc.insert("repos", toml_edit::value(Array::from_iter([url])));
                doc.to_string()
            }
            None => insert_first_setting(text, &format!("repos = {}\n", Array::from_iter([url]))),
        })
    })
}

/// Puts `line` into a config with no top-level settings: after the comments
/// that open the file, up to their last blank line, since comments right
/// above a table describe that table.
fn insert_first_setting(text: &str, line: &str) -> String {
    let mut opening = 0;
    let mut after_blank = 0;
    for l in text.split_inclusive('\n') {
        let trimmed = l.trim();
        if !(trimmed.is_empty() || trimmed.starts_with('#')) {
            break;
        }
        opening += l.len();
        if trimmed.is_empty() {
            after_blank = opening;
        }
    }
    if opening == text.len() {
        // Only comments: the setting goes at the end.
        let newline = if text.is_empty() || text.ends_with('\n') {
            ""
        } else {
            "\n"
        };
        return format!("{text}{newline}{line}");
    }
    let (head, rest) = text.split_at(after_blank);
    format!("{head}{line}\n{rest}")
}

/// Appends `url` after the array's last entry and whatever comments follow
/// it. A multi-line array gets the new entry on its own line, indented like
/// the last entry and followed by a comma.
fn append_formatted(array: &mut Array, url: &str) {
    let text_of =
        |raw: Option<&RawString>| raw.and_then(RawString::as_str).unwrap_or("").to_owned();
    let trailing_comma = array.trailing_comma();
    let mut indent = "  ".to_owned();
    let is_empty = array.is_empty();
    // Everything between the last entry (or its comma) and the closing
    // bracket.
    let mut before_close = text_of(Some(array.trailing()));
    if let Some(last) = array.iter_mut().last() {
        let decor = last.decor_mut();
        if let Some((_, last_line)) = text_of(decor.prefix()).rsplit_once('\n') {
            indent = last_line.to_owned();
        }
        if !trailing_comma {
            before_close = text_of(decor.suffix()) + &before_close;
            decor.set_suffix("");
        }
    }

    let mut value = Value::from(url);
    match before_close.rsplit_once('\n') {
        Some((comments, closing_indent)) => {
            value
                .decor_mut()
                .set_prefix(format!("{comments}\n{indent}"));
            array.set_trailing(format!("\n{closing_indent}"));
            array.set_trailing_comma(true);
        }
        None => {
            value
                .decor_mut()
                .set_prefix(if is_empty { "" } else { " " });
            array.set_trailing(before_close);
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

/// The name in a `hosts` key that is a host, without its port.
enum HostName<'a> {
    /// What the brackets hold, such as `::1` in `[::1]:8080`.
    Bracketed(&'a str),
    /// Such as `github.com` in `github.com:443`.
    Plain(&'a str),
}

/// The name in a `hosts` key, if the key is a host, with a port if it names
/// one, such as `github.com`, `localhost:3000` or `[::1]:8080`. Only a port
/// follows a `:`, so a key such as `user:password` is no host.
fn host_name(key: &str) -> Option<HostName<'_>> {
    let (name, port) = match key.strip_prefix('[') {
        Some(bracketed) => {
            let (ip, port) = bracketed.split_once(']')?;
            let ip_ok = !ip.is_empty()
                && ip
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || ":.".contains(c));
            (ip_ok.then_some(HostName::Bracketed(ip)), port)
        }
        None => {
            let (name, port) = key.split_at(key.find(':').unwrap_or(key.len()));
            let name_ok = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c));
            (name_ok.then_some(HostName::Plain(name)), port)
        }
    };
    let port_ok = port.is_empty()
        || port
            .strip_prefix(':')
            .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()));
    name.filter(|_| port_ok)
}

/// Whether a `hosts` key is a host, as [`host_name`] reads it.
fn is_host(key: &str) -> bool {
    host_name(key).is_some()
}

/// Whether an error may name a `hosts` key. [`is_host`] accepts any run of
/// letters, digits, `.`, `-` and `_`, which is also the shape of a bare
/// token such as `ghp_…` or `glpat-…`, so the key must be a host whose name
/// has a `.`, which no token format in use holds, or is `localhost` or a
/// bracketed IPv6 address.
fn may_name_host(key: &str) -> bool {
    match host_name(key) {
        Some(HostName::Bracketed(ip)) => ip.parse::<Ipv6Addr>().is_ok(),
        Some(HostName::Plain(name)) => name.contains('.') || name.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// A `hosts` key as an error may name it, as a toml key: quoted unless it's
/// a bare key. None unless [`may_name_host`]. A host holds no `"` or `\`
/// that quoting would need to escape.
fn shown_host(key: &str) -> Option<String> {
    if !may_name_host(key) {
        None
    } else if key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c))
    {
        Some(key.to_owned())
    } else {
        Some(format!("\"{key}\""))
    }
}

/// The tokens by host, if every `hosts` key is a host, listed once, with a
/// well-formed token.
fn check_hosts(hosts: BTreeMap<Spanned<String>, HostConfig>) -> Result<Tokens, ConfigError> {
    let mut hosts: Vec<_> = hosts.into_iter().collect();
    // In file order, so a host listed twice is reported where it repeats.
    hosts.sort_by_key(|(host, _)| host.span().start);
    let mut tokens = BTreeMap::new();
    for (host, settings) in hosts {
        if !is_host(host.get_ref()) {
            let name = shown_url(host.get_ref())
                .map(|name| format!("{name:?} "))
                .unwrap_or_default();
            return Err(ConfigError::at(
                &host,
                format!(
                    "`hosts` key {name}must be a host such as \"github.com\", \
                     with a port if the repo URLs name one"
                ),
            ));
        }
        if settings.token.get_ref().is_malformed() {
            return Err(ConfigError::at(
                &settings.token,
                format!(
                    "`hosts.{}.token` must not be empty or contain spaces, \
                     line breaks or control characters",
                    shown_host(host.get_ref()).as_deref().unwrap_or("…")
                ),
            ));
        }
        let lowercased = host.get_ref().to_ascii_lowercase();
        if tokens
            .insert(lowercased, settings.token.into_inner())
            .is_some()
        {
            // The line and column say which host, if it may not be named.
            let named = if may_name_host(host.get_ref()) {
                format!("{:?}", host.get_ref())
            } else {
                "a host".to_owned()
            };
            return Err(ConfigError::at(
                &host,
                format!("`hosts` lists {named} more than once (host names ignore case)"),
            ));
        }
    }
    Ok(Tokens(tokens))
}
