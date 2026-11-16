//! `config.toml`: loading, validation and the default file written on first start.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, bail};
use base64::Engine;
use serde::Deserialize;
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostConfig {
    token: Token,
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
    let raw: RawConfig = toml::from_str(text).map_err(|e| syntax_error(text, &e))?;
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
        tokens: validate_hosts(raw.hosts)?,
    })
}

/// Names the place and the problem but, unlike toml's own message, doesn't
/// quote the line or any string value, either of which may be a token. Names
/// the key instead, if the line sets one.
fn syntax_error(text: &str, error: &toml::de::Error) -> anyhow::Error {
    let message = hide_strings(error.message().trim());
    let Some(span) = error.span() else {
        return anyhow::anyhow!("{message}");
    };
    let start = span.start.min(text.len());
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let line_number = text[..start].matches('\n').count() + 1;
    let column = text[line_start..start].chars().count() + 1;
    let line = text[line_start..].lines().next().unwrap_or_default();
    let place = match line.split_once('=') {
        Some((key, _)) if !key.trim().is_empty() => {
            format!("line {line_number} (`{}`)", key.trim())
        }
        _ => format!("line {line_number}"),
    };
    anyhow::anyhow!("{place}, column {column}: {message}")
}

/// Replaces every double-quoted string in a message, such as the value in
/// `invalid type: string "…", expected a sequence`, with `"…"`.
fn hide_strings(message: &str) -> String {
    let mut hidden = String::new();
    let mut parts = message.split('"');
    hidden.push_str(parts.next().unwrap_or_default());
    // Every other part is inside quotes; an unclosed quote hides the rest.
    while let Some(_quoted) = parts.next() {
        hidden.push_str("\"…\"");
        if let Some(outside) = parts.next() {
            hidden.push_str(outside);
        }
    }
    hidden
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
        let shown = hide_credentials(&repo);
        bail!("`repos` entry {shown:?} {problem}");
    }
    Ok(repo)
}

/// The URL as written, with everything between the scheme and the last `@`
/// replaced by `…`, so a rejection can name the entry without showing a
/// credential in the UI or the log. Any user name and password end at an
/// `@`, so cutting up to the last one also covers a URL that a parser reads
/// differently than its author meant, such as one whose unencoded password
/// holds a `#`.
fn hide_credentials(repo: &str) -> String {
    let (scheme, rest) = match repo.split_once("://") {
        Some((scheme, rest)) => (format!("{scheme}://"), rest),
        None => (String::new(), repo),
    };
    match rest.rfind('@') {
        Some(at) => format!("{scheme}…{}", &rest[at..]),
        None => repo.to_owned(),
    }
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
    /// the log only: it may quote a token.
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

fn validate_hosts(hosts: BTreeMap<String, HostConfig>) -> anyhow::Result<Tokens> {
    let mut tokens = BTreeMap::new();
    for (host, settings) in hosts {
        let valid = !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || ".-_:[]".contains(c));
        if !valid {
            bail!(
                "`hosts` key {host:?} must be a host such as \"github.com\", \
                 with a port if the repo URLs name one"
            );
        }
        if settings.token.is_malformed() {
            bail!(
                "`hosts.\"{host}\".token` must not be empty or contain spaces, \
                 line breaks or control characters"
            );
        }
        if tokens
            .insert(host.to_ascii_lowercase(), settings.token)
            .is_some()
        {
            bail!("`hosts` lists {host:?} more than once (host names ignore case)");
        }
    }
    Ok(Tokens(tokens))
}
