//! Getting archives out: cloning them read-only over git's smart HTTP
//! protocol from `/git/<host>/<path>.git`.

mod common;

use common::forge::FakeForge;
use common::git::{commit_of, git, git_async, preserved_refs, refs, try_git};
use common::{HOUR, TestApp};
use scraper::{Html, Selector};
use tempfile::TempDir;

/// The app's git endpoint URL for a repo on the forge.
fn endpoint(app: &TestApp, forge: &FakeForge, path: &str) -> String {
    format!("http://{}/git/{}/{path}.git", app.addr(), forge.host())
}

#[tokio::test]
async fn a_clone_from_the_endpoint_yields_the_default_branch_and_tags() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo_with_default_branch("foo/bar", "trunk");
    let first = upstream.commit("first");
    upstream.tag("v1");
    let second = upstream.commit("second");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;

    let out = TempDir::new().unwrap();
    git_async(
        out.path(),
        &["clone", "-q", &endpoint(&app, &forge, "foo/bar"), "clone"],
    )
    .await;

    let clone = out.path().join("clone");
    assert_eq!(git(&clone, &["symbolic-ref", "--short", "HEAD"]), "trunk");
    assert_eq!(git(&clone, &["rev-parse", "HEAD"]), second);
    assert_eq!(commit_of(&clone, "refs/tags/v1"), first);
}

#[tokio::test]
async fn ls_remote_on_the_endpoint_shows_preserved_refs_but_not_staging() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let first = upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    let preserved = "refs/preserved/20261116T120000Z/heads/main";
    git(&archive, &["update-ref", preserved, &first]);
    git(&archive, &["update-ref", "refs/staging/heads/main", &first]);

    let out = TempDir::new().unwrap();
    let listing = git_async(
        out.path(),
        &["ls-remote", &endpoint(&app, &forge, "foo/bar")],
    )
    .await;

    assert!(
        listing.contains(&format!("{first}\t{preserved}")),
        "{listing}"
    );
    assert!(listing.contains("refs/heads/main"), "{listing}");
    assert!(!listing.contains("refs/staging/"), "{listing}");
}

#[tokio::test]
async fn a_push_to_the_endpoint_is_refused_and_leaves_the_archive_unchanged() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    let before = refs(&archive);

    let out = TempDir::new().unwrap();
    let url = endpoint(&app, &forge, "foo/bar");
    git_async(out.path(), &["clone", "-q", &url, "clone"]).await;
    let clone = out.path().join("clone");
    std::fs::write(clone.join("file.txt"), "pushed").unwrap();
    git(&clone, &["commit", "-q", "-am", "pushed"]);
    let push = try_git(&clone, &["push", "origin", "HEAD:refs/heads/main"]).await;
    let new_branch = try_git(&clone, &["push", "origin", "HEAD:refs/heads/other"]).await;

    for refused in [&push, &new_branch] {
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(!refused.status.success());
        assert!(stderr.contains("403"), "{stderr}");
    }
    assert_eq!(refs(&archive), before);
}

#[tokio::test]
async fn unknown_repos_traversal_and_non_archive_directories_get_a_404() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let host = forge.host();
    let repos = app.data_dir().join("repos");
    // A plain directory named like an archive, a bare repo outside `repos/`
    // reached through a symlink, and a bare repo directly under `repos/`.
    std::fs::create_dir_all(repos.join(&host).join("plain.git")).unwrap();
    let outside = TempDir::new().unwrap();
    git(outside.path(), &["init", "-q", "--bare", "elsewhere.git"]);
    std::os::unix::fs::symlink(
        outside.path().join("elsewhere.git"),
        repos.join(&host).join("link.git"),
    )
    .unwrap();
    git(&repos, &["init", "-q", "--bare", "top.git"]);
    let refs = "info/refs?service=git-upload-pack";

    assert_eq!(
        app.raw_get_status(&format!("/git/{host}/foo/bar.git/{refs}"))
            .await,
        200,
        "the real archive is served"
    );
    for target in [
        format!("/git/{host}/foo/nope.git/{refs}"),
        format!("/git/{host}/foo/bar/{refs}"),
        format!("/git/{host}/plain.git/{refs}"),
        format!("/git/{host}/link.git/{refs}"),
        format!("/git/top.git/{refs}"),
        format!("/git/{host}/foo/bar.git"),
        "/git/".to_owned(),
        "/git/../config.toml".to_owned(),
        "/git/../state.json".to_owned(),
        format!("/git/{host}/../{host}/foo/bar.git/{refs}"),
        format!("/git/{host}/./foo/bar.git/{refs}"),
        format!("/git/{host}/foo/bar.git/../../../../config.toml"),
        format!("/git/{host}/foo/bar.git/../bar.git/{refs}"),
        "/git/%2e%2e/config.toml".to_owned(),
        "/git/%2E%2E/%2e%2e/config.toml".to_owned(),
        "/git/..%2fconfig.toml".to_owned(),
        format!("/git/{host}/foo%2fbar.git/{refs}"),
        format!("/git/{host}/foo/bar.git/..%2f..%2f..%2f..%2fconfig.toml"),
        format!("/git/{host}/foo/bar.git/%2e%2e/%2e%2e/%2e%2e/%2e%2e/config.toml"),
        format!("/git/{host}/foo/bar.git/config"),
    ] {
        assert_eq!(app.raw_get_status(&target).await, 404, "{target}");
    }
}

#[tokio::test]
async fn an_archived_repos_card_shows_a_clone_line_that_works_as_a_clone_command() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let tip = upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    let lines = async || -> Vec<String> {
        let html = Html::parse_document(&app.get("/").await.text());
        html.select(&Selector::parse("main article code.clone").unwrap())
            .map(|code| code.text().collect())
            .collect()
    };
    assert!(!app.archive(&format!("{}/foo/bar", forge.host())).exists());
    assert_eq!(
        lines().await,
        Vec::<String>::new(),
        "no clone line before there is an archive"
    );

    app.advance_and_settle(24 * HOUR).await;
    let lines = lines().await;
    assert_eq!(
        lines,
        [format!("git clone {}", endpoint(&app, &forge, "foo/bar"))],
    );
    let out = TempDir::new().unwrap();
    let args: Vec<&str> = lines[0].split_whitespace().skip(1).collect();
    git_async(out.path(), &args).await;
    assert_eq!(git(&out.path().join("bar"), &["rev-parse", "HEAD"]), tip);
}

#[tokio::test]
async fn an_untracked_archive_can_still_be_cloned() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let tip = upstream.commit("first");
    let mut app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    app.write_config("repos = []\n");
    app.restart().await;

    let out = TempDir::new().unwrap();
    let listing = git_async(
        out.path(),
        &["ls-remote", &endpoint(&app, &forge, "foo/bar"), "HEAD"],
    )
    .await;

    assert_eq!(listing, format!("{tip}\tHEAD"));
}

#[tokio::test]
async fn a_clone_is_served_while_a_fetch_of_the_same_archive_is_in_progress() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let tip = upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    upstream.commit("second");
    forge.hang("foo/bar");
    app.advance(24 * HOUR);
    forge.wait_for_hung_request().await;

    let out = TempDir::new().unwrap();
    git_async(
        out.path(),
        &["clone", "-q", &endpoint(&app, &forge, "foo/bar"), "clone"],
    )
    .await;

    assert_eq!(git(&out.path().join("clone"), &["rev-parse", "HEAD"]), tip);
}

#[tokio::test]
async fn a_card_with_preserved_refs_shows_a_mirror_clone_line_that_brings_them_along() {
    let forge = FakeForge::start().await;
    let rewritten = forge.create_repo("foo/bar");
    rewritten.commit("first");
    rewritten.tag("v1");
    let app = TestApp::tracking(&[rewritten.url()]).await;
    let bar = endpoint(&app, &forge, "foo/bar");
    // The lines of the pane's card matching `selector`.
    let card_lines = async |selector: &str| -> Vec<String> {
        let html = Html::parse_document(&app.get("/").await.text());
        html.select(&Selector::parse(&format!("main aside article {selector}")).unwrap())
            .map(|code| code.text().collect())
            .collect()
    };
    app.advance_and_settle(24 * HOUR).await;
    assert_eq!(card_lines("code.clone").await, [format!("git clone {bar}")]);
    assert_eq!(
        card_lines("code.mirror-clone").await,
        Vec::<String>::new(),
        "only a card with preserved refs offers a mirror clone"
    );

    rewritten.force_push_new_history("rewritten");
    rewritten.move_tag("v1");
    app.advance_and_settle(24 * HOUR).await;
    let preserved = preserved_refs(&app.archive(&format!("{}/foo/bar", forge.host())));
    assert_eq!(preserved.len(), 2, "{preserved:?}");

    assert_eq!(
        card_lines("code.clone").await,
        [format!("git clone {bar}")],
        "the plain clone line is unchanged"
    );
    assert_eq!(
        card_lines(".preserved code.mirror-clone").await,
        [format!("git clone --mirror {bar}")],
    );
    let html = Html::parse_document(&app.get("/").await.text());
    let mirror_clone = card_lines(".preserved code.mirror-clone").await.remove(0);
    let button = html
        .select(&Selector::parse(".preserved .clone-line button[data-copy]").unwrap())
        .next()
        .expect("the mirror line has a copy button");
    assert_eq!(
        button.value().attr("data-copy"),
        Some(mirror_clone.as_str())
    );

    let out = TempDir::new().unwrap();
    let args: Vec<&str> = mirror_clone.split_whitespace().skip(1).collect();
    git_async(out.path(), &args).await;
    assert_eq!(preserved_refs(&out.path().join("bar.git")), preserved);
}

/// An app with one archived repo that has a preserved ref, so its card shows
/// both a plain and a mirror clone line.
async fn app_with_a_preserved_ref() -> (FakeForge, TestApp) {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    let first = upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url()]).await;
    app.advance_and_settle(24 * HOUR).await;
    let archive = app.archive(&format!("{}/foo/bar", forge.host()));
    git(
        &archive,
        &[
            "update-ref",
            "refs/preserved/20261116T120000Z/heads/main",
            &first,
        ],
    );
    (forge, app)
}

/// Asserts that, for a request carrying these headers, the list page's plain
/// and mirror clone lines both use `scheme`.
async fn assert_clone_scheme(headers: &[(&str, &str)], scheme: &str) {
    let (forge, app) = app_with_a_preserved_ref().await;
    assert_clone_scheme_on(&app, &forge, headers, scheme).await;
}

async fn assert_clone_scheme_on(
    app: &TestApp,
    forge: &FakeForge,
    headers: &[(&str, &str)],
    scheme: &str,
) {
    let html = Html::parse_document(&app.get_with_headers("/", headers).await.text());
    let line = |selector: &str| -> String {
        html.select(&Selector::parse(selector).unwrap())
            .next()
            .unwrap_or_else(|| panic!("no {selector} on the list page"))
            .text()
            .collect()
    };
    let http_url = endpoint(app, forge, "foo/bar");
    let url = format!("{scheme}{}", http_url.strip_prefix("http").unwrap());
    assert_eq!(
        line("code.clone"),
        format!("git clone {url}"),
        "behind {headers:?}"
    );
    assert_eq!(
        line("code.mirror-clone"),
        format!("git clone --mirror {url}"),
        "behind {headers:?}"
    );
}

/// Asserts `scheme` for each set of headers, all against one app.
async fn assert_clone_schemes(cases: &[(&[(&str, &str)], &str)]) {
    let (forge, app) = app_with_a_preserved_ref().await;
    for (headers, scheme) in cases {
        assert_clone_scheme_on(&app, &forge, headers, scheme).await;
    }
}

#[tokio::test]
async fn clone_lines_use_https_behind_a_proxy_that_sets_x_forwarded_proto() {
    assert_clone_scheme(&[("X-Forwarded-Proto", "https")], "https").await;
}

#[tokio::test]
async fn clone_lines_use_https_behind_a_proxy_that_sets_forwarded_proto() {
    assert_clone_schemes(&[
        (&[("Forwarded", "proto=https")], "https"),
        (&[("Forwarded", "for=1.2.3.4;proto=\"HTTPS\"")], "https"),
        (&[("Forwarded", "for=1.2.3.4 ; Proto = https")], "https"),
    ])
    .await;
}

#[tokio::test]
async fn clone_lines_use_the_scheme_of_the_hop_nearest_the_client() {
    assert_clone_schemes(&[
        (&[("X-Forwarded-Proto", "https, http")], "https"),
        (&[("Forwarded", "proto=https, proto=http")], "https"),
        (&[("Forwarded", "for=1.2.3.4, proto=https")], "http"),
    ])
    .await;
}

#[tokio::test]
async fn forwarded_wins_over_x_forwarded_proto() {
    assert_clone_schemes(&[
        (
            &[("Forwarded", "proto=http"), ("X-Forwarded-Proto", "https")],
            "http",
        ),
        (
            &[("Forwarded", "for=1.2.3.4"), ("X-Forwarded-Proto", "https")],
            "https",
        ),
    ])
    .await;
}

#[tokio::test]
async fn clone_lines_stay_on_http_without_a_usable_forwarded_scheme() {
    assert_clone_schemes(&[
        (&[], "http"),
        (&[("X-Forwarded-Proto", "ftp")], "http"),
        (&[("X-Forwarded-Proto", "")], "http"),
        (&[("Forwarded", "proto=")], "http"),
        (&[("Forwarded", "proto=ftp")], "http"),
    ])
    .await;
}
