//! `/metrics`: Prometheus text exposition format, version 0.0.4.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::identity::RepoId;
use crate::overview::View;
use crate::state::{RepoState, Status};

pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Renders every metric. Per-repo metrics cover every repo in the view.
/// `preserved_refs` holds the number of preserved refs
/// in each of their archives, by repo identity, for the repos that have one.
pub fn render(overview: &View, preserved_refs: &BTreeMap<String, usize>) -> String {
    // Each repo with whether it's tracked and its state, if it has any.
    let repos: Vec<(&RepoId, bool, Option<&RepoState>)> = overview
        .repos
        .iter()
        .map(|r| (&r.id, r.tracked, r.state.as_ref()))
        .collect();
    let per_repo = |value: fn(&RepoState) -> Option<f64>| {
        repos
            .iter()
            .filter_map(move |(id, _, state)| {
                Some((vec![("repo", id.as_str())], value((*state)?)?))
            })
            .collect::<Vec<_>>()
    };
    let mut out = String::new();
    family(
        &mut out,
        "archiver_repo_status",
        "gauge",
        "1 for the repo's current status, 0 for the others.",
        repos
            .iter()
            .filter_map(|(id, _, state)| Some((id, state.as_ref()?.status?)))
            .flat_map(|(id, current)| {
                Status::ALL.map(|status| {
                    let labels = vec![("repo", id.as_str()), ("status", status.as_str())];
                    (labels, if status == current { 1.0 } else { 0.0 })
                })
            }),
    );
    family(
        &mut out,
        "archiver_repo_last_success_timestamp_seconds",
        "gauge",
        "When the repo was last fetched successfully, in Unix seconds.",
        per_repo(|s| s.last_success.map(unix_seconds)),
    );
    family(
        &mut out,
        "archiver_repo_size_bytes",
        "gauge",
        "Size of the repo's archive on disk, measured after each fetch.",
        per_repo(|s| s.size_bytes.map(|bytes| bytes as f64)),
    );
    family(
        &mut out,
        "archiver_repo_preserved_refs",
        "gauge",
        "Preserved refs in the repo's archive: tips kept from before rewrites.",
        repos.iter().filter_map(|(id, _, _)| {
            let count = preserved_refs.get(id.as_str())?;
            Some((vec![("repo", id.as_str())], *count as f64))
        }),
    );
    family(
        &mut out,
        "archiver_repo_tracked",
        "gauge",
        "1 if the repo is tracked, 0 for an archive kept after untracking.",
        repos.iter().map(|(id, tracked, _)| {
            (
                vec![("repo", id.as_str())],
                if *tracked { 1.0 } else { 0.0 },
            )
        }),
    );
    family(
        &mut out,
        "archiver_next_fetch_timestamp_seconds",
        "gauge",
        "When the next fetch is due, in Unix seconds.",
        overview.next_fetch.map(|t| (vec![], unix_seconds(t))),
    );
    family(
        &mut out,
        "archiver_fetch_duration_seconds",
        "gauge",
        "How long the last fetch took.",
        overview
            .last_fetch_duration
            .map(|d| (vec![], d.as_secs_f64())),
    );
    let errors = &overview.fetch_errors;
    family(
        &mut out,
        "archiver_fetch_errors_total",
        "counter",
        "Failed fetches since the app started, by the status they lead to.",
        [
            (Status::Unavailable, errors.unavailable),
            (Status::FetchFailed, errors.fetch_failed),
        ]
        .map(|(status, count)| (vec![("status", status.as_str())], count as f64)),
    );
    family(
        &mut out,
        "archiver_rewrites_total",
        "counter",
        "Fetches that found a rewrite upstream and preserved the old tips, since the app started.",
        [(vec![], overview.rewrites as f64)],
    );
    out
}

/// One metric family: its HELP and TYPE lines, then a sample per value.
fn family<'a>(
    out: &mut String,
    name: &str,
    kind: &str,
    help: &str,
    samples: impl IntoIterator<Item = (Vec<(&'a str, &'a str)>, f64)>,
) {
    let _ = writeln!(out, "# HELP {name} {help}");
    let _ = writeln!(out, "# TYPE {name} {kind}");
    for (labels, value) in samples {
        let _ = write!(out, "{name}");
        for (i, (label, label_value)) in labels.iter().enumerate() {
            let open = if i == 0 { '{' } else { ',' };
            let _ = write!(out, "{open}{label}=\"{}\"", escape_label(label_value));
        }
        if !labels.is_empty() {
            out.push('}');
        }
        let _ = writeln!(out, " {value}");
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
