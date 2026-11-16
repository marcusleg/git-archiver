//! `/metrics`: Prometheus text exposition format, version 0.0.4.

use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::overview::Overview;

pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

pub fn render(overview: &Overview) -> String {
    let per_repo = |value: fn(&crate::state::RepoState) -> Option<f64>| {
        overview
            .tracked_with_state()
            .filter_map(move |(t, state)| Some((Some(t.id.as_str()), value(state?)?)))
            .collect::<Vec<_>>()
    };
    let mut out = String::new();
    gauge(
        &mut out,
        "archiver_repo_last_success_timestamp_seconds",
        "When the repo was last fetched successfully, in Unix seconds.",
        per_repo(|s| s.last_success.map(unix_seconds)),
    );
    gauge(
        &mut out,
        "archiver_repo_size_bytes",
        "Size of the repo's archive on disk, measured after each fetch.",
        per_repo(|s| s.size_bytes.map(|bytes| bytes as f64)),
    );
    gauge(
        &mut out,
        "archiver_next_fetch_timestamp_seconds",
        "When the next fetch is due, in Unix seconds.",
        overview.next_fetch.map(|t| (None, unix_seconds(t))),
    );
    gauge(
        &mut out,
        "archiver_fetch_duration_seconds",
        "How long the last fetch took.",
        overview
            .last_fetch_duration
            .map(|d| (None, d.as_secs_f64())),
    );
    out
}

/// One gauge family: its HELP and TYPE lines, then a sample per value,
/// labelled with the repo identity if it has one.
fn gauge<'a>(
    out: &mut String,
    name: &str,
    help: &str,
    samples: impl IntoIterator<Item = (Option<&'a str>, f64)>,
) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} gauge");
    for (repo, value) in samples {
        match repo {
            Some(repo) => {
                let _ = writeln!(out, "{name}{{repo=\"{}\"}} {value}", escape_label(repo));
            }
            None => {
                let _ = writeln!(out, "{name} {value}");
            }
        }
    }
}

fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn unix_seconds(time: SystemTime) -> f64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
