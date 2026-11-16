//! `config.toml`: loading, validation and the default file written on first start.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::net::SocketAddr;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use base64::Engine;
use toml::Spanned;
use toml::de::{DeTable, DeValue};
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

/// An access token for one host. Its `Debug` output hides it, so it can't
/// end up in a log by accident.
#[derive(Clone, PartialEq, Eq)]
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
/// is built only from text the app knows and quotes nothing from the file
/// (ADR 0003). See [`ConfigError`].
fn parse(text: &str) -> anyhow::Result<Config> {
    check(text).map_err(|error| anyhow::anyhow!(error.shown(text)))
}

/// The config a file's text holds, or the first error in it.
fn check(text: &str) -> Result<Config, ConfigError> {
    let doc = DeTable::parse(text).map_err(|e| ConfigError::while_parsing(&e))?;
    let raw = read_settings(doc.get_ref())?;
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

/// The names the app gives its settings, which an error may always show.
/// This and [`HOST_FIELDS`] are the app's whole schema: [`read_settings`]
/// accepts these names and no others.
const SETTINGS: [&str; 4] = ["interval", "listen", "repos", "hosts"];

/// The fields of a `hosts` entry.
const HOST_FIELDS: [&str; 1] = ["token"];

/// The config as written, with where each value the app checks itself sits,
/// so that its error can say where.
struct RawConfig {
    interval: Spanned<String>,
    listen: Spanned<String>,
    repos: Vec<Spanned<String>>,
    hosts: Vec<(Spanned<String>, Spanned<Token>)>,
}

type Entry<'a, 'de> = (
    &'a Spanned<std::borrow::Cow<'de, str>>,
    &'a Spanned<DeValue<'de>>,
);

/// The entries of `table` in file order.
fn in_file_order<'a, 'de>(table: &'a DeTable<'de>) -> Vec<Entry<'a, 'de>> {
    let mut entries: Vec<_> = table.iter().collect();
    entries.sort_by_key(|(key, _)| key.span().start);
    entries
}

/// Checks the parsed file against the app's schema: its settings, their
/// types and the fields of each `hosts` entry. The first problem in the file
/// is the error. Whatever it finds, it words itself.
fn read_settings(root: &DeTable<'_>) -> Result<RawConfig, ConfigError> {
    let mut interval = Spanned::new(0..0, "24h".to_owned());
    let mut listen = Spanned::new(0..0, "0.0.0.0:8080".to_owned());
    let mut repos = Vec::new();
    let mut hosts = Vec::new();
    for (key, value) in in_file_order(root) {
        match key.get_ref().as_ref() {
            "interval" => interval = string_at(value, &["interval"])?,
            "listen" => listen = string_at(value, &["listen"])?,
            "repos" => {
                let DeValue::Array(entries) = value.get_ref() else {
                    return Err(ConfigError::wrong_type(value, &["repos"], "a sequence"));
                };
                for entry in entries {
                    repos.push(string_at(entry, &["repos"])?);
                }
            }
            "hosts" => {
                let DeValue::Table(entries) = value.get_ref() else {
                    return Err(ConfigError::wrong_type(value, &["hosts"], "a map"));
                };
                for (host, settings) in in_file_order(entries) {
                    hosts.push(read_host(host, settings)?);
                }
            }
            name => return Err(ConfigError::unknown_field(key, &[name], &SETTINGS)),
        }
    }
    Ok(RawConfig {
        interval,
        listen,
        repos,
        hosts,
    })
}

/// A `hosts` entry: the host and its token.
fn read_host(
    host: &Spanned<std::borrow::Cow<'_, str>>,
    settings: &Spanned<DeValue<'_>>,
) -> Result<(Spanned<String>, Spanned<Token>), ConfigError> {
    let name = host.get_ref().as_ref();
    let DeValue::Table(fields) = settings.get_ref() else {
        return Err(ConfigError::wrong_type(
            settings,
            &["hosts", name],
            "struct HostConfig",
        ));
    };
    let mut token = None;
    for (field, value) in in_file_order(fields) {
        match field.get_ref().as_ref() {
            "token" => {
                let text = string_at(value, &["hosts", name, "token"])?;
                token = Some(Spanned::new(text.span(), Token(text.into_inner())));
            }
            other => {
                return Err(ConfigError::unknown_field(
                    field,
                    &["hosts", name, other],
                    &HOST_FIELDS,
                ));
            }
        }
    }
    let Some(token) = token else {
        return Err(ConfigError::at_key(
            settings.span(),
            &["hosts", name],
            "missing field `token`".to_owned(),
        ));
    };
    Ok((Spanned::new(host.span(), name.to_owned()), token))
}

/// The string at `value`, which is at `path`.
fn string_at(value: &Spanned<DeValue<'_>>, path: &[&str]) -> Result<Spanned<String>, ConfigError> {
    match value.get_ref() {
        DeValue::String(text) => Ok(Spanned::new(value.span(), text.to_string())),
        _ => Err(ConfigError::wrong_type(value, path, "a string")),
    }
}

/// A config error, as only [`parse`] shows it: the line and column it starts
/// at, the key it's about if that's known and may be shown, and the problem.
struct ConfigError {
    span: Option<Range<usize>>,
    /// As [`shown_key`] shows it.
    key: Option<String>,
    /// Text the app wrote, or for a syntax error toml's with
    /// [`hide_file_text`] applied.
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

    /// A problem with the value at `span`, which is at `path`.
    fn at_key(span: Range<usize>, path: &[&str], problem: String) -> Self {
        Self {
            span: Some(span),
            key: shown_key(path),
            problem,
        }
    }

    /// A syntax error, found before the file parsed. It names no key, as
    /// there is no parsed file to take one from, and a line that looks like
    /// it sets one may well be inside a string. It is the one error in toml's
    /// words, with [`hide_file_text`] applied.
    fn while_parsing(error: &toml::de::Error) -> Self {
        Self {
            span: error.span(),
            key: None,
            problem: hide_file_text(error.message()),
        }
    }

    /// A key that isn't one of `known`, the last of `path`. It says what was
    /// expected, but not what was found.
    fn unknown_field(
        key: &Spanned<std::borrow::Cow<'_, str>>,
        path: &[&str],
        known: &[&str],
    ) -> Self {
        let names: Vec<String> = known.iter().map(|name| format!("`{name}`")).collect();
        let expected = match names.as_slice() {
            [only] => only.clone(),
            _ => format!("one of {}", names.join(", ")),
        };
        // The key is shown only if it is one of the app's names, such as
        // `interval` written inside a `hosts` entry.
        let shown = path.last().is_some_and(|name| is_app_name(name));
        let found = if shown { path[path.len() - 1] } else { "…" };
        Self::at_key(
            key.span(),
            path,
            format!("unknown field `{found}`, expected {expected}"),
        )
    }

    /// A value at `path` that isn't of the type the schema wants. It names
    /// the type found, not the value.
    fn wrong_type(value: &Spanned<DeValue<'_>>, path: &[&str], expected: &str) -> Self {
        let found = match value.get_ref() {
            DeValue::String(_) => "string \"…\"",
            DeValue::Integer(_) => "integer `…`",
            DeValue::Float(_) => "float `…`",
            DeValue::Boolean(_) => "boolean `…`",
            DeValue::Datetime(_) => "date and time `…`",
            DeValue::Array(_) => "sequence",
            DeValue::Table(_) => "map",
        };
        Self::at_key(
            value.span(),
            path,
            format!("invalid type: {found}, expected {expected}"),
        )
    }

    /// The error as the app shows it, such as
    /// ``line 3 (`hosts.….…`), column 1: unknown field `…`, expected `token` ``,
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

/// What toml's syntax errors quote in backticks, none of it from the file:
/// exactly the literals toml_parser 1.1.5 can say it expected (its
/// `Expected::Literal`s), which are punctuation, escape letters, number
/// prefixes and the words `true`, `false`, `inf` and `nan`. A newline it
/// names in words, outside backticks.
const TOML_TOKENS: [&str; 30] = [
    "#", "'", ",", "-", "=", "[", "]", "]]", "{", "}", "_", "\\", "\"", "\"\"\"", "'''", "0b",
    "0o", "0x", "b", "e", "f", "n", "r", "u", "U", "x", "true", "false", "inf", "nan",
];

/// Whether `name` is one the app gave a setting or a field.
fn is_app_name(name: &str) -> bool {
    SETTINGS.contains(&name) || HOST_FIELDS.contains(&name)
}

/// A key as an error may show it, such as `hosts.….token`: each part that is
/// one of the app's own names, and `…` for any other part, which may be a
/// credential. None if no part may be shown.
fn shown_key(path: &[&str]) -> Option<String> {
    let mut any_shown = false;
    let parts: Vec<&str> = path
        .iter()
        .enumerate()
        .map(|(i, &part)| {
            // A `hosts` key is a host, whatever it's called.
            let is_host = i == 1 && path[0] == "hosts";
            let shown = !is_host && is_app_name(part);
            any_shown |= shown;
            if shown { part } else { "…" }
        })
        .collect();
    any_shown.then(|| parts.join("."))
}

/// toml's syntax error text with every piece it quotes from the file shown
/// as `…`: each double-quoted string, and each backtick-quoted piece other
/// than [`TOML_TOKENS`].
fn hide_file_text(message: &str) -> String {
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
        if quote == "`" && TOML_TOKENS.contains(&piece) {
            shown.push_str(&format!("`{piece}`"));
        } else {
            shown.push_str(&format!("{quote}…{quote}"));
        }
        rest = end.map_or("", |end| &quoted[end + 1..]);
    }
    shown.push_str(rest);
    shown
}

/// Where the string toml wrote ends: at the first `"` no `\` escapes.
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
/// accept it. An error names the entry by its position only.
fn check_repo_entry(position: usize, repo: Spanned<String>) -> Result<String, ConfigError> {
    if let Err(problem) = check_repo_url(repo.get_ref()) {
        return Err(ConfigError::at(
            &repo,
            format!("`repos` entry {position} {problem}"),
        ));
    }
    Ok(repo.into_inner())
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

/// A change to the config's `repos` list.
pub enum RepoListChange<'a> {
    /// Append `url`, whose identity is `id`.
    Add { url: &'a str, id: &'a RepoId },
    /// Remove every entry that shares the archive of `id`.
    Remove { id: &'a RepoId },
}

/// Why the repo list couldn't be changed. None of these quotes text from the
/// config (ADR 0003), apart from [`EditError::AlreadyTracked`], which carries
/// the tracked URL for the caller to show.
#[derive(Debug)]
pub enum EditError {
    /// The file doesn't hold a valid config right now, or the edit didn't
    /// come out as intended. Its own error is for the log only.
    Invalid(anyhow::Error),
    /// Reading or writing the file failed.
    Io(anyhow::Error),
    /// Adding only: a repo with the same archive is tracked under this URL.
    AlreadyTracked(String),
}

/// What a repo-list edit did to the file.
#[derive(Debug, PartialEq, Eq)]
pub enum Edited {
    Changed,
    /// Removing only: the config didn't list the repo, so the file is left
    /// alone.
    NotListed,
}

/// Held while the config is read for its repo list or changed, so an edit and
/// a reload never interleave.
static CONFIG_LOCK: Mutex<()> = Mutex::new(());

/// Loads the config and hands the result to `f`, with no edit in between.
pub fn load_locked<T>(path: &Path, f: impl FnOnce(anyhow::Result<Config>) -> T) -> T {
    let _lock = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    f(load(path))
}

/// Checks that the repo could be added now: the config is valid and doesn't
/// track a repo with the same archive yet. Returns the token to check
/// upstream with, if the repo's host has one.
pub fn check_add(path: &Path, id: &RepoId) -> Result<Option<Token>, EditError> {
    let (_, config) = read_for_edit(path)?;
    refuse_if_tracked(&config, id)?;
    Ok(config.tokens.for_repo(id).cloned())
}

/// Applies `change` to `repos`, then calls `on_edited` with the resulting
/// repo list (also when nothing changed) before any other edit or reload can
/// read the file. The file is edited in place, so the operator's comments and
/// formatting survive, the result is checked to list exactly the expected
/// repos, and the file is replaced in one step, so a crash never leaves a
/// partial config.
pub fn edit_repo_list(
    path: &Path,
    change: RepoListChange<'_>,
    on_edited: impl FnOnce(&[String]),
) -> Result<Edited, EditError> {
    let _lock = CONFIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // A symlinked config is replaced at its target, keeping the link.
    let path = std::fs::canonicalize(path)
        .with_context(|| format!("couldn't read config {}", path.display()))
        .map_err(EditError::Io)?;
    let (text, config) = read_for_edit(&path)?;

    let (edited, expected) = match change {
        RepoListChange::Add { url, id } => {
            refuse_if_tracked(&config, id)?;
            let mut expected = config.repos;
            expected.push(url.to_owned());
            (append_repo(&text, url), expected)
        }
        RepoListChange::Remove { id } => {
            let expected: Vec<String> = config
                .repos
                .iter()
                .filter(|repo| !shares_archive(repo, id))
                .cloned()
                .collect();
            if expected.len() == config.repos.len() {
                on_edited(&config.repos);
                return Ok(Edited::NotListed);
            }
            (
                remove_entries(&text, |repo| shares_archive(repo, id)),
                expected,
            )
        }
    };
    let edited = edited.map_err(EditError::Invalid)?;
    let repos = parse(&edited)
        .context("the edited config doesn't parse")
        .map_err(EditError::Invalid)?
        .repos;
    if repos != expected {
        // The lists are the operator's text, so they aren't quoted.
        return Err(EditError::Invalid(anyhow::anyhow!(
            "the edited config doesn't list the expected repos ({} listed, {} expected)",
            repos.len(),
            expected.len()
        )));
    }
    write_atomically(&path, &edited).map_err(EditError::Io)?;
    on_edited(&repos);
    Ok(Edited::Changed)
}

/// `text` as an editable document. toml_edit's own error quotes the line it
/// stopped at, so it is replaced by one that quotes nothing.
fn edit_document(text: &str) -> anyhow::Result<DocumentMut> {
    text.parse()
        .map_err(|_| anyhow::anyhow!("the config couldn't be edited in place"))
}

/// The config text without the `repos` entries `remove` picks, keeping the
/// file's line endings.
fn remove_entries(text: &str, remove: impl Fn(&str) -> bool) -> anyhow::Result<String> {
    keeping_line_endings(text, |text| {
        let mut doc = edit_document(text)?;
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

/// Whether the config entry `repo` has the archive of `id`.
fn shares_archive(repo: &str, id: &RepoId) -> bool {
    RepoId::from_url(repo).is_ok_and(|other| other.same_archive(id))
}

/// The config's text and contents.
fn read_for_edit(path: &Path) -> Result<(String, Config), EditError> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("couldn't read config {}", path.display()))
        .map_err(EditError::Io)?;
    let config = parse(&text).map_err(EditError::Invalid)?;
    Ok((text, config))
}

/// Refuses a repo whose archive the config already tracks. URLs differing
/// only in scheme share an archive, so they count as the same repo.
fn refuse_if_tracked(config: &Config, id: &RepoId) -> Result<(), EditError> {
    let tracked = config.repos.iter().find(|repo| shares_archive(repo, id));
    match tracked {
        Some(existing) => Err(EditError::AlreadyTracked(existing.clone())),
        None => Ok(()),
    }
}

/// The config text with `url` appended to `repos`, laid out like the entries
/// around it and keeping the file's line endings.
fn append_repo(text: &str, url: &str) -> anyhow::Result<String> {
    keeping_line_endings(text, |text| {
        let mut doc = edit_document(text)?;
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

/// Whether a `hosts` key is a host, with a port if it names one, such as
/// `github.com`, `localhost:3000` or `[::1]:8080`. Only a port follows a
/// `:`, so a key such as `user:password` is no host.
fn is_host(key: &str) -> bool {
    let (name_ok, port) = match key.strip_prefix('[') {
        Some(bracketed) => {
            let Some((ip, port)) = bracketed.split_once(']') else {
                return false;
            };
            let ip_ok = !ip.is_empty()
                && ip
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || ":.".contains(c));
            (ip_ok, port)
        }
        None => {
            let (name, port) = key.split_at(key.find(':').unwrap_or(key.len()));
            let name_ok = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c));
            (name_ok, port)
        }
    };
    let port_ok = port.is_empty()
        || port
            .strip_prefix(':')
            .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()));
    name_ok && port_ok
}

/// The tokens by host, if every `hosts` key is a host, listed once, with a
/// well-formed token. An error never names the host: its line and column
/// say which.
fn check_hosts(hosts: Vec<(Spanned<String>, Spanned<Token>)>) -> Result<Tokens, ConfigError> {
    let mut tokens = BTreeMap::new();
    for (host, settings) in hosts {
        if !is_host(host.get_ref()) {
            return Err(ConfigError::at(
                &host,
                "`hosts` key must be a host such as \"github.com\", \
                 with a port if the repo URLs name one",
            ));
        }
        if settings.get_ref().is_malformed() {
            return Err(ConfigError::at(
                &settings,
                "`hosts.….token` must not be empty or contain spaces, \
                 line breaks or control characters",
            ));
        }
        let lowercased = host.get_ref().to_ascii_lowercase();
        if tokens.insert(lowercased, settings.into_inner()).is_some() {
            return Err(ConfigError::at(
                &host,
                "`hosts` lists a host more than once (host names ignore case)",
            ));
        }
    }
    Ok(Tokens(tokens))
}

#[cfg(test)]
mod tests {
    use super::edit_document;

    #[test]
    fn a_document_that_cannot_be_edited_quotes_nothing_from_it() {
        let error =
            edit_document("repos = [\"https://ghp_secret@example.com/a/b\"\nx = ").unwrap_err();
        assert!(!format!("{error:#}").contains("ghp_secret"));
    }
}
