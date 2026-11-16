//! Repo identity: the normalized URL that names a repo's archive, its state
//! entry and its metrics label.

use std::fmt;
use std::path::PathBuf;

use anyhow::{Context, bail};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoId {
    /// `<scheme>://<host>[:<port>]/<path>`
    id: String,
    /// `<host>[:<port>]/<path>`
    host_and_path: String,
}

impl RepoId {
    /// Lowercases the scheme and host and drops trailing slashes and a
    /// trailing `.git`. The path keeps its case.
    pub fn from_url(raw: &str) -> anyhow::Result<Self> {
        let url = url::Url::parse(raw).with_context(|| format!("{raw:?} is not a valid URL"))?;
        let Some(host) = url.host_str().filter(|h| !h.is_empty()) else {
            bail!("{raw:?} has no host");
        };
        let host = host.to_ascii_lowercase();
        let host = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host,
        };
        let path = url.path().trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let path = path.trim_matches('/');
        if path.is_empty() {
            bail!("{raw:?} has no repository path");
        }
        let segments: Vec<&str> = path.split('/').collect();
        for (i, segment) in segments.iter().enumerate() {
            let last = i + 1 == segments.len();
            // Each segment becomes a directory under `repos/`, so none may
            // escape it or land inside another archive's `.git` directory.
            if matches!(*segment, "" | "." | "..") || (!last && segment.ends_with(".git")) {
                bail!("{raw:?} has an unsupported repository path");
            }
        }
        Ok(Self {
            id: format!("{}://{host}/{path}", url.scheme().to_ascii_lowercase()),
            host_and_path: format!("{host}/{path}"),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.id
    }

    /// `<host>[:<port>]/<path>`, as in the archive's path and its git
    /// endpoint URL.
    pub fn host_and_path(&self) -> &str {
        &self.host_and_path
    }

    /// Where the archive lives, relative to `repos/`.
    pub fn archive_path(&self) -> PathBuf {
        PathBuf::from(format!("{}.git", self.host_and_path))
    }
}

impl fmt::Display for RepoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.id)
    }
}
