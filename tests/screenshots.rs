//! The README's screenshots of the list page, at a desktop and a phone
//! viewport. Ignored by `cargo test`; `scripts/update-screenshots.sh` runs it
//! and writes the screenshots to `docs/screenshots/`.
//!
//! The scene is made the way an operator's would be: the app fetches repos
//! from fake forges, sees a rewrite, a takedown and an outage, and a repo is
//! untracked. The forges listen on random local ports, so before the
//! screenshots the data directory is moved to real-looking hosts, the way an
//! operator moves an archive by copying it.

mod common;

use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use common::browser::{Browser, Viewport};
use common::forge::{Failure, FakeForge, Upstream};
use common::{HOUR, MINUTE, TestApp, config_tracking};

/// Where the README links the screenshots from, relative to the repo root.
const OUTPUT_DIR: &str = "docs/screenshots";

/// The URL the browser shows the app under, so the clone commands on the
/// page read as they would for someone following the README.
const APP_URL: &str = "http://localhost:8080/";

#[tokio::test]
#[ignore = "writes the README screenshots; run scripts/update-screenshots.sh"]
async fn readme_screenshots() {
    // Commits with fixed dates have the same SHAs on every run, so the
    // screenshots change only when the page does.
    // SAFETY: nothing else runs yet that could read the environment.
    unsafe {
        std::env::set_var("GIT_AUTHOR_DATE", "2026-11-01T09:00:00Z");
        std::env::set_var("GIT_COMMITTER_DATE", "2026-11-01T09:00:00Z");
    }

    let github = FakeForge::start().await;
    let codeberg = FakeForge::start().await;
    let gitlab = FakeForge::start().await;

    let aurora = repo(&codeberg, "aurora/aurora-sync", 14, &["v1.0.0", "v1.1.0"]);
    let tiles = repo(&github, "nightjar/tiles", 9, &["v0.4.2"]);
    let orbit = repo(&github, "orbit-dev/orbit", 22, &["v2.0.0", "v2.1.0"]);
    let grabber = repo(&github, "pixelmint/yt-grabber", 17, &["2026.10.30"]);
    let lantern = repo(&github, "quietwave/lantern", 6, &[]);
    let legacy = repo(&github, "sundial/sundial-legacy", 4, &["v0.9.0"]);
    let docs = repo(&codeberg, "tessel/tessel-docs", 8, &[]);
    let manifest = repo(&gitlab, "harbor-tools/manifest", 11, &["v3.2.0"]);

    let tracked = [&aurora, &tiles, &orbit, &grabber, &lantern, &manifest];
    let urls: Vec<&str> = tracked.iter().map(|r| r.url()).collect();
    let mut all = urls.clone();
    all.extend([legacy.url(), docs.url()]);

    // A first day: every repo is cloned.
    let mut app = TestApp::tracking(&all).await;
    fetch_each_once(&app, all.len()).await;

    // Upstream changes overnight, and two repos are untracked by hand, which
    // keeps their archives.
    aurora.force_push_new_history("Start over from a clean history");
    aurora.move_tag("v1.1.0");
    tiles.commit("Fix tile seams at fractional zoom");
    orbit.commit("Add orbit completions for fish");
    github.fail("pixelmint/yt-grabber", Failure::DmcaTakedown);
    github.fail("quietwave/lantern", Failure::ServerError);
    manifest.commit("Bump the manifest schema to v4");
    app.write_config(&config_tracking(&urls));

    // A second day: every tracked repo is fetched again, and the page is
    // looked at a little after the last fetch.
    fetch_each_once(&app, urls.len()).await;
    app.advance_and_settle(37 * MINUTE).await;

    app.stop().await;
    move_host(app.data_dir(), &github.host(), "github.com");
    move_host(app.data_dir(), &codeberg.host(), "codeberg.org");
    move_host(app.data_dir(), &gitlab.host(), "gitlab.com");
    // The clock stands still from here on, and the next fetch is hours
    // away, so the app never reaches out to the real hosts.
    app.start_again().await;

    let output = Path::new(env!("CARGO_MANIFEST_DIR")).join(OUTPUT_DIR);
    std::fs::create_dir_all(&output).unwrap();
    let now = UNIX_EPOCH + Duration::from_secs(app.now());
    // The whole page on a desktop, and the first screenful on a phone.
    for (viewport, name, whole_page) in [
        (Viewport::Desktop, "desktop.png", true),
        (Viewport::Phone, "mobile.png", false),
    ] {
        let browser = Browser::launch_for_screenshots(viewport, app.addr()).await;
        let tab = browser.open_at(APP_URL, now).await;
        tab.sees("aurora-sync").await;
        // Show the rewrite the archive preserved, in the pane beside the
        // list. A phone shows the list alone.
        if let Viewport::Desktop = viewport {
            tab.run("document.querySelectorAll('details.preserved').forEach((d) => d.open = true)")
                .await;
            tab.sees("refs/preserved/").await;
        }
        tab.save_screenshot(&output.join(name), whole_page).await;
        println!("wrote {OUTPUT_DIR}/{name}");
    }
}

/// Lets a day pass, the default interval, in which the app fetches each of
/// its `tracked` repos once, a fetch every `interval ÷ tracked`.
async fn fetch_each_once(app: &TestApp, tracked: usize) {
    let gap = 24 * HOUR / u32::try_from(tracked).unwrap();
    for _ in 0..tracked {
        app.advance_and_settle(gap).await;
    }
}

/// An upstream repo at `path` with `commits` commits and these tags, on the
/// last commit.
fn repo(forge: &FakeForge, path: &str, commits: usize, tags: &[&str]) -> Upstream {
    let upstream = forge.create_repo(path);
    for n in 1..=commits {
        upstream.commit(&format!("Change {n} to {path}"));
    }
    for tag in tags {
        upstream.tag(tag);
    }
    upstream
}

/// Moves every archive, state entry and tracked repo of the stopped app from
/// the forge at `forge` to `https://<host>/`: URLs and git's messages in
/// `config.toml` and `state.json` name the new host, and the archives move to
/// `repos/<host>/`. Tracked URLs lose their `.git`, as people usually write
/// them.
fn move_host(data_dir: &Path, forge: &str, host: &str) {
    let from = format!("http://{forge}/");
    let to = format!("https://{host}/");

    let config_path = data_dir.join("config.toml");
    let config = std::fs::read_to_string(&config_path).unwrap();
    let config: String = config
        .lines()
        .map(|line| match line.contains(&from) {
            true => line.replace(&from, &to).replace(".git\"", "\"") + "\n",
            false => format!("{line}\n"),
        })
        .collect();
    std::fs::write(&config_path, config).unwrap();

    let state_path = data_dir.join("state.json");
    let state = std::fs::read_to_string(&state_path).unwrap();
    std::fs::write(&state_path, state.replace(&from, &to)).unwrap();

    let repos = data_dir.join("repos");
    std::fs::rename(repos.join(forge), repos.join(host)).unwrap();
}
