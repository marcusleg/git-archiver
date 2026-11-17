//! Every repo's own page at `/repos/<identity>`: the list with that repo
//! selected and its card in the pane.

mod common;

use common::app_page::{AppPage, select, select_in, text};
use common::forge::FakeForge;
use common::{HOUR, MINUTE, TestApp};

const BAR: &str = "https://github.com/foo/bar";
const PROJECT: &str = "https://gitlab.com/group/sub/project.git";

async fn two_repos() -> TestApp {
    TestApp::tracking(&[PROJECT, BAR]).await
}

/// The configured URLs of the rows marked selected, across both tables.
fn selected(list: &AppPage) -> Vec<String> {
    list.tracked()
        .into_iter()
        .chain(list.untracked())
        .filter(|row| row.selected)
        .map(|row| row.title)
        .collect()
}

#[tokio::test]
async fn each_row_links_to_its_repos_page() {
    let app = two_repos().await;

    let rows = app.list_page().await.tracked();

    let links: Vec<_> = rows.iter().map(|r| r.href.as_str()).collect();
    assert_eq!(
        links,
        [
            "/repos/github.com/foo/bar",
            "/repos/gitlab.com/group/sub/project"
        ]
    );
}

#[tokio::test]
async fn a_repo_page_selects_that_repo_and_names_it_in_the_title() {
    let app = two_repos().await;

    let list = app.page("/repos/gitlab.com/group/sub/project").await;

    assert_eq!(list.pane_title().as_deref(), Some(PROJECT));
    assert_eq!(selected(&list), [PROJECT]);
    assert_eq!(list.title(), "gitlab.com/group/sub/project · Git Archiver");
    assert_eq!(list.tracked().len(), 2, "the page lists every repo");
}

#[tokio::test]
async fn the_list_page_still_selects_the_first_tracked_repo() {
    let app = two_repos().await;

    let list = app.list_page().await;

    assert_eq!(list.pane_title().as_deref(), Some(BAR));
    assert_eq!(selected(&list), [BAR]);
    assert_eq!(list.title(), "Git Archiver");
}

#[tokio::test]
async fn other_spellings_of_a_repos_identity_select_the_same_repo() {
    let app = two_repos().await;

    for spelling in [
        "/repos/GitLab.COM/group/sub/project",
        "/repos/gitlab.com/group/sub/project.git",
        "/repos/gitlab.com/group/sub/project/",
        "/repos/gitlab.com/group/sub/project.git/",
    ] {
        let page = app.get(spelling).await;
        assert_eq!(page.status, 200, "{spelling}: {}", page.text());
        assert_eq!(
            page.path, "/repos/gitlab.com/group/sub/project",
            "{spelling} leads to the canonical URL"
        );
        let list = AppPage::parse(&page.text());
        assert_eq!(list.pane_title().as_deref(), Some(PROJECT), "{spelling}");
        assert_eq!(selected(&list), [PROJECT], "{spelling}");
    }
}

#[tokio::test]
async fn an_unknown_repo_is_not_found_with_a_link_back_to_the_list() {
    let app = two_repos().await;

    for path in [
        "/repos/github.com/foo/other",
        // The path of an identity keeps its case.
        "/repos/github.com/Foo/Bar",
        // Not an identity at all.
        "/repos/github.com",
        "/repos/github.com/foo/..",
    ] {
        let page = app.get(path).await;
        assert_eq!(page.status, 404, "{path}");
        let list = AppPage::parse(&page.text());
        let links: Vec<_> = select(&list.html, "main a")
            .iter()
            .map(|a| a.value().attr("href").unwrap_or_default().to_owned())
            .collect();
        assert!(links.contains(&"/".to_owned()), "{path}: {}", page.text());
        assert!(list.pane_card().is_none(), "{path}");
    }
}

#[tokio::test]
async fn only_get_is_served_on_a_repos_page() {
    let app = two_repos().await;

    let page = app
        .post_form("/repos/github.com/foo/bar", &[("url", "x")])
        .await;

    assert_eq!(page.status, 405);
}

#[tokio::test]
async fn an_untracked_archives_page_shows_its_read_only_card() {
    let forge = FakeForge::start().await;
    let upstream = forge.create_repo("foo/bar");
    upstream.commit("first");
    let app = TestApp::tracking(&[upstream.url(), BAR]).await;
    app.advance_and_settle(12 * HOUR).await;
    app.write_config(&common::config_tracking(&[BAR]));
    app.advance_and_settle(MINUTE).await;
    let id = format!("http://{}/foo/bar", forge.host());

    let row = app
        .list_page()
        .await
        .untracked()
        .into_iter()
        .find(|row| row.title == id)
        .expect("the archive is listed as untracked");
    let list = app.page(&row.href).await;

    assert_eq!(row.href, format!("/repos/{}/foo/bar", forge.host()));
    assert_eq!(list.pane_title().as_deref(), Some(id.as_str()));
    assert_eq!(selected(&list), [id]);
    let card = list.pane_card().unwrap();
    assert!(select_in(&card, "form").is_empty(), "{}", card.html());
    assert!(select_in(&card, "button[data-dialog]").is_empty());
    assert!(
        select_in(&card, ".clone")
            .iter()
            .any(|c| text(c).contains("/foo/bar.git")),
        "the archive can still be cloned: {}",
        card.html()
    );
}

#[tokio::test]
async fn untracking_from_a_repos_page_leads_back_to_the_list_whatever_happens_to_the_archive() {
    for choice in ["Keep archive", "Delete archive"] {
        let app = two_repos().await;

        let page = app
            .untrack_from_page("/repos/gitlab.com/group/sub/project", choice)
            .await;

        assert_eq!(page.status, 200, "{choice}: {}", page.text());
        assert_eq!(page.path, "/", "{choice}");
        let list = AppPage::parse(&page.text());
        assert_eq!(list.title(), "Git Archiver");
        assert_eq!(list.titles("Tracked repos"), [BAR], "{choice}");
    }
}

#[tokio::test]
async fn a_failed_untrack_shows_that_repos_page_with_the_error() {
    let app = two_repos().await;
    let list = app.page("/repos/gitlab.com/group/sub/project").await;
    let card = list.pane_card().unwrap();
    let repo = select_in(&card, "dialog input[name=repo]")[0]
        .value()
        .attr("value")
        .unwrap()
        .to_owned();
    app.write_config("repos = [\n");

    let page = app
        .post_form("/repos/untrack", &[("repo", &repo), ("archive", "keep")])
        .await;

    assert_eq!(page.status, 422);
    let list = AppPage::parse(&page.text());
    let alert = text(&select(&list.html, "main > [role=alert]")[0]);
    assert!(alert.contains("Couldn’t untrack the repo"), "{alert}");
    assert_eq!(list.pane_title().as_deref(), Some(PROJECT));
    assert_eq!(selected(&list), [PROJECT]);
    assert_eq!(list.title(), "gitlab.com/group/sub/project · Git Archiver");
}
