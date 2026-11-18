//! Links to a repo's pages upstream: the repo itself on any host, and its
//! branches and commits on the forges whose URL layout we know.

use crate::identity::RepoId;

/// The repo's page upstream: its identity, which has no `.git` suffix or
/// trailing slashes.
pub fn repo_url(id: &RepoId) -> String {
    id.as_str().to_owned()
}

/// The branch's page upstream, or `None` on a host whose layout we don't
/// know.
pub fn branch_url(id: &RepoId, branch: &str) -> Option<String> {
    let layout = Layout::of(id)?;
    Some(page_url(id, layout.branch, branch.split('/')))
}

/// The commit's page upstream, or `None` on a host whose layout we don't
/// know.
pub fn commit_url(id: &RepoId, sha: &str) -> Option<String> {
    let layout = Layout::of(id)?;
    Some(page_url(id, layout.commit, [sha]))
}

/// Where a forge puts a repo's branch and commit pages: the path segments
/// between the repo and a branch name or a commit's SHA.
struct Layout {
    branch: &'static [&'static str],
    commit: &'static [&'static str],
}

impl Layout {
    /// The layout of the forge `id` lives on, matched by host and port
    /// exactly. Self-hosted forges can lay out their pages any way, so only
    /// these hosts get branch and commit links.
    fn of(id: &RepoId) -> Option<Self> {
        let (branch, commit): (&[&str], &[&str]) = match id.host() {
            "github.com" => (&["tree"], &["commit"]),
            "gitlab.com" => (&["-", "tree"], &["-", "commit"]),
            "codeberg.org" => (&["src", "branch"], &["commit"]),
            _ => return None,
        };
        Some(Self { branch, commit })
    }
}

/// The repo's URL followed by `prefix` and `segments`, each segment
/// percent-encoded on its own.
fn page_url<'a>(
    id: &RepoId,
    prefix: &[&str],
    segments: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut url = url::Url::parse(id.as_str()).expect("an identity is a URL");
    url.path_segments_mut()
        .expect("an identity's URL has a path")
        .extend(prefix)
        .extend(segments);
    url.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity of `url`, which must be valid.
    fn id(url: &str) -> RepoId {
        RepoId::from_url(url).unwrap()
    }

    #[test]
    fn a_repo_links_to_its_url_without_the_git_suffix() {
        assert_eq!(
            repo_url(&id("https://github.com/owner/repo.git")),
            "https://github.com/owner/repo"
        );
        assert_eq!(
            repo_url(&id("https://git.example.org/team/repo/")),
            "https://git.example.org/team/repo"
        );
    }

    #[test]
    fn a_branch_links_to_its_page_on_each_known_forge() {
        assert_eq!(
            branch_url(&id("https://github.com/owner/repo.git"), "main").as_deref(),
            Some("https://github.com/owner/repo/tree/main")
        );
        assert_eq!(
            branch_url(&id("https://gitlab.com/group/sub/project"), "main").as_deref(),
            Some("https://gitlab.com/group/sub/project/-/tree/main")
        );
        assert_eq!(
            branch_url(&id("https://codeberg.org/owner/repo"), "main").as_deref(),
            Some("https://codeberg.org/owner/repo/src/branch/main")
        );
    }

    #[test]
    fn a_branch_name_keeps_its_slashes_and_encodes_the_rest() {
        assert_eq!(
            branch_url(&id("https://github.com/owner/repo"), "feature/x").as_deref(),
            Some("https://github.com/owner/repo/tree/feature/x")
        );
        assert_eq!(
            branch_url(&id("https://github.com/owner/repo"), "fix/50%#1 ü?").as_deref(),
            Some("https://github.com/owner/repo/tree/fix/50%25%231%20%C3%BC%3F")
        );
    }

    #[test]
    fn a_branch_on_an_unknown_host_has_no_link() {
        assert_eq!(branch_url(&id("https://git.example.org/a/b"), "main"), None);
        assert_eq!(branch_url(&id("https://github.com:8443/a/b"), "main"), None);
    }

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn a_commit_links_to_its_full_sha_page_on_each_known_forge() {
        assert_eq!(
            commit_url(&id("https://github.com/owner/repo.git"), SHA).as_deref(),
            Some("https://github.com/owner/repo/commit/0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(
            commit_url(&id("https://gitlab.com/group/sub/project"), SHA).as_deref(),
            Some(
                "https://gitlab.com/group/sub/project/-/commit/0123456789abcdef0123456789abcdef01234567"
            )
        );
        assert_eq!(
            commit_url(&id("https://codeberg.org/owner/repo"), SHA).as_deref(),
            Some("https://codeberg.org/owner/repo/commit/0123456789abcdef0123456789abcdef01234567")
        );
    }

    #[test]
    fn a_commit_on_an_unknown_host_has_no_link() {
        assert_eq!(commit_url(&id("https://git.example.org/a/b"), SHA), None);
    }
}
