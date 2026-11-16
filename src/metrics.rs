//! `/metrics`: Prometheus text exposition format, version 0.0.4.

use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::scheduler::Snapshot;

pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

pub fn render(snapshot: &Snapshot) -> String {
    let mut out = String::new();
    let tracked = || {
        snapshot
            .tracked
            .iter()
            .filter_map(|t| Some((t.id.as_str(), snapshot.state.repos.get(t.id.as_str())?)))
    };

    family(
        &mut out,
        "archiver_repo_last_success_timestamp_seconds",
        "When the repo was last fetched successfully, in Unix seconds.",
    );
    for (repo, state) in tracked() {
        if let Some(time) = state.last_success {
            sample(
                &mut out,
                "archiver_repo_last_success_timestamp_seconds",
                Some(repo),
                unix_seconds(time),
            );
        }
    }

    family(
        &mut out,
        "archiver_repo_size_bytes",
        "Size of the repo's archive on disk, measured after each fetch.",
    );
    for (repo, state) in tracked() {
        if let Some(bytes) = state.size_bytes {
            sample(
                &mut out,
                "archiver_repo_size_bytes",
                Some(repo),
                bytes as f64,
            );
        }
    }

    family(
        &mut out,
        "archiver_next_fetch_timestamp_seconds",
        "When the next fetch is due, in Unix seconds.",
    );
    if let Some(next) = snapshot.next_fetch {
        sample(
            &mut out,
            "archiver_next_fetch_timestamp_seconds",
            None,
            unix_seconds(next),
        );
    }

    family(
        &mut out,
        "archiver_fetch_duration_seconds",
        "How long the last fetch took.",
    );
    if let Some(duration) = snapshot.last_fetch_duration {
        sample(
            &mut out,
            "archiver_fetch_duration_seconds",
            None,
            duration.as_secs_f64(),
        );
    }
    out
}

fn family(out: &mut String, name: &str, help: &str) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} gauge");
}

fn sample(out: &mut String, name: &str, repo: Option<&str>, value: f64) {
    match repo {
        Some(repo) => {
            let _ = writeln!(out, "{name}{{repo=\"{}\"}} {value}", escape_label(repo));
        }
        None => {
            let _ = writeln!(out, "{name} {value}");
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
