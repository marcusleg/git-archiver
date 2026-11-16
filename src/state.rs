//! `state.json`: what the app remembers about each repo between fetches.
//! Anything git can answer (tips, default branch) is read from the archive
//! instead.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    /// By repo identity. Entries outlive tracking.
    #[serde(default)]
    pub repos: BTreeMap<String, RepoState>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoState {
    #[serde(default, with = "rfc3339", skip_serializing_if = "Option::is_none")]
    pub last_attempt: Option<SystemTime>,
    #[serde(default, with = "rfc3339", skip_serializing_if = "Option::is_none")]
    pub last_success: Option<SystemTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Status>,
    #[serde(default, with = "rfc3339", skip_serializing_if = "Option::is_none")]
    pub status_since: Option<SystemTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The last fetch succeeded.
    Available,
}

pub fn file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("state.json")
}

/// Loads the state, or an empty one if there is no file yet.
pub fn load(path: &Path) -> anyhow::Result<State> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(State::default()),
        Err(e) => return Err(e).with_context(|| format!("couldn't read state {}", path.display())),
    };
    serde_json::from_str(&text).with_context(|| format!("invalid state {}", path.display()))
}

impl State {
    /// Writes to a temp file and renames it into place, so a crash never
    /// leaves a partial file behind.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        let context = || format!("couldn't write state {}", path.display());
        let dir = path.parent().expect("state path has a parent");
        let mut file = tempfile::NamedTempFile::new_in(dir).with_context(context)?;
        serde_json::to_writer_pretty(&mut file, self).with_context(context)?;
        file.write_all(b"\n")
            .and_then(|()| file.as_file().sync_all())
            .with_context(context)?;
        file.persist(path)
            .map_err(|e| e.error)
            .with_context(context)?;
        Ok(())
    }
}

/// Times as RFC 3339 strings in UTC, to the second.
mod rfc3339 {
    use std::time::SystemTime;

    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(time: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
        match time {
            Some(time) => s.serialize_str(&humantime::format_rfc3339_seconds(*time).to_string()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SystemTime>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|text| humantime::parse_rfc3339(&text).map_err(D::Error::custom))
            .transpose()
    }
}
