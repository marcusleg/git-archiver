# Git archiving: prior art, completeness, takedown detection, constraints, and legal context

Background research for the `git-archiver` design interview. It reports facts and trade-offs and makes no design decisions.

- **Researched:** 2026-11-16.
- **Sources:** primary sources only, meaning official docs, project source code and READMEs, specs, and statute text. Where only a secondary or community source exists, the claim is marked **[lower confidence]**.
- **Observed:** some claims come from live probes run on 2026-11-16 against `api.github.com`, `github.com`, `gitlab.com`, and `archive.softwareheritage.org` with `curl`, `git` 2.56.1, and `gh`. These are marked **[observed]**. They are snapshots of behaviour and can change without notice.

---

## Summary: the most decision-relevant findings

1. **GitHub signals a DMCA takedown precisely, but only over REST.** `GET /repos/{owner}/{repo}` returns **HTTP 451** with `{"message":"Repository access blocked","block":{"reason":"dmca","created_at":…,"html_url":"<link to the notice in github/dmca>"}}` **[observed]**. This matches the [2016 changelog](https://developer.github.com/changes/2016-03-17-the-451-status-code-is-now-supported). The current ["Get a repository" docs](https://docs.github.com/en/rest/repos/repos#get-a-repository) list only 200/301/403/404. **GraphQL returns the same `NOT_FOUND` for a DMCA-blocked repo as for a deleted one** **[observed]**, so GraphQL alone cannot tell the two apart.
2. **The `github/dmca` feed arrives after the takedown, not before it.** For 5 full-repo takedowns sampled from October 2026, the notice was committed to [`github/dmca`](https://github.com/github/dmca) between about 35 seconds and 8 minutes *after* the `block.created_at` timestamp **[observed]**. A watcher on that feed cannot save the named repos. It can still save **forks and copies the notice did not name**, because [GitHub does not automatically disable forks](https://docs.github.com/en/site-policy/content-removal-policies/dmca-takedown-policy#b-what-about-forks-or-whats-a-fork) unless the notice covers the whole network.
3. **The value of an archive is in having copied the repo *before* it is at risk.** Software Heritage had no record of 4 of 5 recently DMCA'd repos, and the fifth had last been visited in 2020 **[observed]**. Passive, third-party crawling does not reliably cover small or new repos.
4. **`git clone --mirror` is necessary but far from complete.** It takes every ref the server advertises, which on GitHub includes `refs/pull/*/head` and `refs/pull/*/merge` **[observed]**. It does **not** take LFS objects, submodule repos, the wiki repo (`<repo>.wiki.git`), issues, PRs as conversations, reviews, releases and their assets, discussions, or forks' own branches. Each of those needs a separate API or git fetch ([git-clone](https://git-scm.com/docs/git-clone), [git-lfs-fetch](https://github.com/git-lfs/git-lfs/blob/main/docs/man/git-lfs-fetch.adoc), [GitHub wiki docs](https://docs.github.com/en/communities/documenting-your-project-with-wikis/adding-or-editing-wiki-pages)).
5. **Pulling LFS objects spends the *repo owner's* bandwidth quota.** GitHub bills LFS downloads to the owner (10 GiB/month on Free). When that runs out, LFS is disabled for the account until the next month ([GitHub docs](https://docs.github.com/en/repositories/working-with-files/managing-large-files/about-storage-and-bandwidth-usage)). Archiving LFS at scale can therefore hurt the projects being archived.
6. **No existing tool combines watch, capture-everything, and survive.** python-github-backup has the widest metadata capture but restores nothing and only covers GitHub ([README](https://github.com/josegonzalez/python-github-backup)). Forgejo/Gitea can import issues, PRs, and releases **only once**: choosing "mirror" drops all metadata ([Forgejo source](https://codeberg.org/forgejo/forgejo/src/commit/10615cce5019acefd625c1300cba85d228b045cc/routers/web/repo/migrate.go#L234-L241)). ghorg and gickup are git-centric. Software Heritage keeps code only. Radicle needs the repo to be on Radicle.
7. **Mirror-style sync destroys history by default.** Forgejo pull mirrors run `git fetch --prune` with pruning on by default ([source](https://codeberg.org/forgejo/forgejo/src/commit/10615cce5019acefd625c1300cba85d228b045cc/services/repository/migrate.go#L194)). `git clone --mirror` sets up a refspec under which "all these refs are overwritten" on update ([git-clone](https://git-scm.com/docs/git-clone)). ghorg's `--prune` deletes local clones of repos that were removed on the remote ([sample-conf](https://github.com/gabrie30/ghorg/blob/master/sample-conf.yaml)). An archive that faithfully follows its upstream also follows a force-push or deletion.
8. **GitHub's policies name archiving explicitly, but also cap bulk use.** AUP §7 says "Archivists may use public information from the Service for archival purposes." AUP §9 still allows GitHub to throttle or suspend for excessive bandwidth, and ToS §H allows API suspension for "excessively frequent requests" ([AUP](https://docs.github.com/en/site-policy/acceptable-use-policies/github-acceptable-use-policies), [ToS](https://docs.github.com/en/site-policy/github-terms/github-terms-of-service#h-api-terms)). Unauthenticated HTTPS clones are rate-limited, and GitHub may temporarily require authentication for some public clones ([2025 changelog](https://github.blog/changelog/2025-05-08-updated-rate-limits-for-unauthenticated-requests/), [2026 changelog](https://github.blog/changelog/2026-10-19-some-git-over-https-may-temporarily-require-authentication)).
9. **Pushing an archive back to a forge puts it under that forge's takedown process.** The `github/dmca` README says re-posting content that was taken down, without following the process, violates GitHub's DMCA policy ([README](https://github.com/github/dmca/blob/master/README.md)). Codeberg's terms allow only content you have the right to share, take down content "deemed illegal in Germany", and may remove inactive migrations or forks that have no unique contributions ([Codeberg ToU](https://codeberg.org/Codeberg/org/src/branch/main/TermsOfUse.md)). Its FAQ calls pull mirrors "problematic" ([FAQ](https://docs.codeberg.org/getting-started/faq/)).
10. **§512 gives a 10–14 business-day restore path, and the youtube-dl case shows it is real.** A counter-notice leads to restoration unless the claimant files suit ([17 U.S.C. §512(g)(2)(C)](https://www.law.cornell.edu/uscode/text/17/512)). youtube-dl was taken down on 2020-10-23 on a §1201 claim and reinstated on 2020-11-16 ([notice](https://github.com/github/dmca/blob/master/2020/10/2020-10-23-RIAA.md), [reversal](https://github.com/github/dmca/blob/master/2020/11/2020-11-16-RIAA-reversal.md), [GitHub blog](https://github.blog/news-insights/policy-news-and-insights/standing-up-for-developers-youtube-dl-is-back/)). Takedowns can be temporary, so "unavailable" is a state that can change back.
11. **The fork network shares one object store, which cuts both ways.** "Commits pushed to any repository in a network can be accessible from other repositories in that network", "even after a fork is deleted" ([GitHub docs](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/working-with-forks/about-permissions-and-visibility-of-forks)). When a public parent is deleted, "an active public fork becomes the new upstream" ([GitHub docs](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/working-with-forks/what-happens-to-forks-when-a-repository-is-deleted-or-changes-visibility)). A DMCA notice can still disable the whole network ([github/dmca README](https://github.com/github/dmca/blob/master/README.md)).
12. **Decentralised distribution exists, but someone has to keep holding the data.** IPFS content survives only while some node pins it ([IPFS docs](https://docs.ipfs.tech/concepts/persistence/)). Mutable BitTorrent updates (BEP 46) are still a draft ([BEP 46](https://www.bittorrent.org/beps/bep_0046.html)). Git bundles are single files built for offline transfer and can be incremental ([git-bundle](https://git-scm.com/docs/git-bundle)). Bundle URIs let a server send clients to static or CDN bundle files ([bundle-uri](https://git-scm.com/docs/bundle-uri)).

---

## 1. Prior art

### 1.1 Software Heritage (SWH)

**What it is.** A non-profit archive run under Inria. It "collects publicly available source code, and its development history, from a variety of sources", does "not perform any screening", and operates under French law ([SWH content policy](https://archive.softwareheritage.org/content-policy/)).

**How saving works.**
- *Save Code Now API:* `POST /api/1/origin/save/(visit_type)/url/(origin_url)/`. Supported visit types are `bzr`, `cvs`, `git`, `hg`, `svn`, and `tarball`. The response carries `save_request_status` (accepted, rejected, or pending) and `save_task_status` (not created through succeeded or failed) ([API doc](https://archive.softwareheritage.org/api/1/origin/save/doc/)). Requests for `github.com/cli/cli` show `accepted` and `succeeded` **[observed]**.
- *Webhooks:* SWH has push-webhook receivers for GitHub, GitLab, Bitbucket, Gitea, and SourceForge, so a maintainer can have the archive updated on every push ([SWH webhook docs](https://docs.softwareheritage.org/_sources/user/save_code_now/webhooks/index.rst.txt)).
- *Rate limits:* 120 requests/hour anonymous and 1200/hour authenticated with a bearer token ([SWH API](https://archive.softwareheritage.org/api/)). The API returns `X-Ratelimit-*` headers **[observed]**.
- *Manual review:* submissions can stall in manual review, especially for non-GitHub origins. This comes from Joey Hess's blog, not SWH ([joeyh.name](https://joeyh.name/blog/entry/Withrawing_github-backup/)) **[lower confidence]**.

**What it stores.**
- *Git data:* git objects plus a "snapshot" of branches per visit. The git loader **drops** `refs/pull/*/merge`, `refs/merge-requests/*/merge`, `refs/pipelines/*`, `refs/changes/*`, and peeled `^{}` refs. It **keeps** `refs/pull/*/head` ([`swh-loader-git` utils.py L95–112 @ da7c9eba](https://gitlab.softwareheritage.org/swh/devel/swh-loader-git/-/blob/da7c9eba3c737541b7a1834d0391b2f9d935d453/swh/loader/git/utils.py#L95-112)).
- *Not stored:* no git-lfs handling turned up in the loader source (grep for "lfs" found nothing) **[observed, absence of evidence]**. Issues, PRs as conversations, releases' binary assets, and wikis as forge features are outside the stated scope of "source code and development history" ([content policy](https://archive.softwareheritage.org/content-policy/)).

**Retrieval.** The Vault rebuilds objects into downloadable bundles asynchronously ([vault getting started](https://docs.softwareheritage.org/_sources/devel/swh-vault/getting-started.rst.txt)).
- `POST /api/1/vault/git-bare/<swhid>/` starts a job, and you poll until it is done.
- A **snapshot** SWHID produces a bare repo with "all branches and releases contained in the snapshot" ([git-bare doc](https://archive.softwareheritage.org/api/1/vault/git-bare/doc/)).

**Takedowns.** SWH has a takedown process under French law ([content policy](https://archive.softwareheritage.org/content-policy/)).
- Removal runs through the `swh alter` tooling ([ops how-to](https://docs.softwareheritage.org/_sources/sysadm/operations/takedown-requests/howto-process-takedown-requests.rst.txt)).
- Mirror operators get notified and decide under their own local law. This comes from the [mirror takedown doc](https://docs.softwareheritage.org/sysadm/mirror-operations/takedown-notices.html), seen only as a search snippet **[lower confidence]**.

**Shortfalls for at-risk repos.**
- Coverage is uneven: 4 of 5 repos DMCA'd in October 2026 were "Origin … not found", and one had last been visited on 2020-06-10 **[observed]**.
- It stores code only, with no issues, PRs, releases, or LFS.
- It is a central service that can also be asked to remove content.
- Retrieval is asynchronous through the Vault.

### 1.2 Radicle

**What it is.** A peer-to-peer git network. "Radicle repositories are simply Git repositories stored in a special location on disk." Each peer gets a namespace under `refs/namespaces/`, so that "only one copy of each object is stored across all repository namespaces" ([protocol guide](https://radicle.dev/guides/protocol)). The `radicle.xyz` domain now redirects to `radicle.dev` **[observed]**.

**What it replicates.** Code, plus "Collaborative Objects" (COBs: Issues, Patches, Identities) that are "stored directly inside a repository, and replicated between peers" ([protocol guide](https://radicle.dev/guides/protocol)).

**Replication model.**
- Each node has a **seeding policy** that sets "the rules for data retention, and synchronization" ([protocol guide](https://radicle.dev/guides/protocol)).
- `rad seed <RID>` replicates a repo, and `--scope all` takes every peer's refs ([user guide](https://radicle.dev/guides/user)).
- The `iris.radicle.network` and `rosa.radicle.network` seed nodes automatically seed all public repos during the network's early stages ([user guide](https://radicle.dev/guides/user)).
- Nodes are identified by Ed25519 keys and find repos through gossip ([protocol guide](https://radicle.dev/guides/protocol)).

**Shortfalls.**
- A repo enters Radicle through `rad init` in a local git repo. The guide says to publish only repos "you own or maintain" ([user guide](https://radicle.dev/guides/user)). There is no documented way to seed a GitHub repo that its owner never published to Radicle.
- No GitHub issue or PR import is documented ([user guide](https://radicle.dev/guides/user)). gickup lists Radicle as a *destination*, with issue recreation marked "[COMING SOON]" ([gickup conf.example.yml](https://github.com/cooperspencer/gickup/blob/main/conf.example.yml)).
- The protocol guide's privacy section was removed after a security vulnerability ([protocol guide](https://radicle.dev/guides/protocol)).
- There is no network-wide deletion, but nothing guarantees that anyone keeps seeding a repo either.

### 1.3 ghorg

**What it is.** A Go CLI to "quickly clone all of an orgs, or users repos into a single directory". It lists "Creating backups" as a use case ([README](https://github.com/gabrie30/ghorg)). It supports GitHub, GitLab, Bitbucket, Gitea, Codeberg/Forgejo, and Sourcehut (limited).

**What it captures.**
- `--backup` uses `git clone --mirror`. `--clone-wiki` adds wikis and `--include-submodules` adds submodules ([README §Creating Backups](https://github.com/gabrie30/ghorg#creating-backups)).
- `--fetch-git-lfs` runs `git lfs fetch --all`, and `--skip-forks` filters out forks ([sample-conf.yaml](https://github.com/gabrie30/ghorg/blob/master/sample-conf.yaml)).
- It has cron and HTTP-server scheduling, plus clone metrics ([README](https://github.com/gabrie30/ghorg)).

**Shortfalls.**
- No issues, PRs, releases, or assets.
- By default a re-run does a `git pull` and `git clean` that overwrites local state ([README](https://github.com/gabrie30/ghorg)).
- `--prune` deletes local clones of repos that were removed on the remote ([sample-conf.yaml](https://github.com/gabrie30/ghorg/blob/master/sample-conf.yaml)), which is the opposite of what an archive wants.
- It is designed around org and user scopes, not hand-picked at-risk repos.

### 1.4 Gitea / Forgejo: pull mirrors, migration, and `dump-repo`

These are verified against the Forgejo source at commit `10615cce` (2026-11-16). Gitea is the upstream and is likely similar, but it was not separately verified.

**Pull mirrors** sync "branches, tags, and commits" periodically ([Forgejo docs](https://forgejo.org/docs/latest/user/repo-mirror/)).
- **Refs:** the initial clone uses `git clone --mirror` ([modules/git/repo.go](https://codeberg.org/forgejo/forgejo/src/commit/10615cce5019acefd625c1300cba85d228b045cc/modules/git/repo.go)). Each sync runs `git fetch [--prune] --tags <remote>` ([mirror_pull.go L350–355](https://codeberg.org/forgejo/forgejo/src/commit/10615cce5019acefd625c1300cba85d228b045cc/services/mirror/mirror_pull.go#L350-L355)).
- **Pruning:** `EnablePrune: true` is the default for new mirrors ([services/repository/migrate.go L194](https://codeberg.org/forgejo/forgejo/src/commit/10615cce5019acefd625c1300cba85d228b045cc/services/repository/migrate.go#L194)).
- **Interval:** the default is 8 h and the minimum 10 min ([modules/setting/mirror.go](https://codeberg.org/forgejo/forgejo/src/commit/10615cce5019acefd625c1300cba85d228b045cc/modules/setting/mirror.go)).
- **`refs/pull/*`:** the sync loop skips these refs when it generates sync events ("Discard GitHub pull requests", [mirror_pull.go L587](https://codeberg.org/forgejo/forgejo/src/commit/10615cce5019acefd625c1300cba85d228b045cc/services/mirror/mirror_pull.go#L587)). Whether they stay on disk or show in the UI was not verified.

**Migration (one-shot)** can import wiki, issues, milestones, labels, releases, comments, PRs, release assets, and LFS ([MigrateOptions](https://codeberg.org/forgejo/forgejo/src/commit/10615cce5019acefd625c1300cba85d228b045cc/modules/migration/options.go#L11)). PR reviews are converted as well ([github.go](https://codeberg.org/forgejo/forgejo/src/commit/10615cce5019acefd625c1300cba85d228b045cc/services/migrations/github.go)). Sources include GitHub, GitLab, Gitea/Forgejo, Gogs, OneDev, GitBucket, Codebase, Pagure, and Bitbucket DC ([services/migrations/](https://codeberg.org/forgejo/forgejo/src/commit/10615cce5019acefd625c1300cba85d228b045cc/services/migrations); also [Codeberg docs](https://docs.codeberg.org/advanced/migrating-repos/)).
- **If "mirror" is selected, Issues, Milestones, Labels, Comments, PullRequests, and Releases are forced to `false`** ([routers/web/repo/migrate.go L234–241](https://codeberg.org/forgejo/forgejo/src/commit/10615cce5019acefd625c1300cba85d228b045cc/routers/web/repo/migrate.go#L234-L241)).
- The result is either a living git mirror or a one-time metadata snapshot, never both.

**`forgejo dump-repo`** is a CLI that dumps a repo from git, GitHub, Gitea, or GitLab into a local directory. Its units are "wiki, issues, labels, releases, release_assets, milestones, pull_requests, comments". `restore-repo` loads a dump back in ([cmd/dump_repo.go](https://codeberg.org/forgejo/forgejo/src/commit/10615cce5019acefd625c1300cba85d228b045cc/cmd/dump_repo.go#L76-L79)). It is effectively an offline, forge-neutral export format.

**Push mirrors** run `git push --mirror` and force-push. "LFS over SSH protocol is not implemented in Forgejo, any LFS objects will not be mirrored" ([Forgejo docs](https://forgejo.org/docs/latest/user/repo-mirror/)).

**Shortfalls.**
- Metadata is never kept up to date.
- Prune is on by default.
- An instance can disable new pull mirrors entirely (`DisableNewPull`, [migrate.go L162](https://codeberg.org/forgejo/forgejo/src/commit/10615cce5019acefd625c1300cba85d228b045cc/routers/web/repo/migrate.go#L162)).
- Codeberg discourages pull mirrors (see §4).

### 1.5 `git clone --mirror` and `git bundle`

**`--mirror`** "implies `--bare`" and "maps all refs (including remote-tracking branches, notes etc.) and sets up a refspec configuration such that all these refs are overwritten by a `git remote update`". `--recurse-submodules` "is ignored if the cloned repository does not have a worktree/checkout (i.e. if any of `--no-checkout`/`-n`, `--bare`, or `--mirror` is given)" ([git-clone](https://git-scm.com/docs/git-clone)).

**`git bundle`** is used for "the 'offline' transfer of Git objects without an active 'server'" ([git-bundle](https://git-scm.com/docs/git-bundle)).
- `git bundle create backup.bundle --all` is a full-ref backup, though "only for the refs". It leaves out config, hooks, and stash.
- `old..new` bundles are incremental but require their prerequisites to be present.
- `git bundle verify` checks a bundle, and `git clone backup.bundle` restores one.
- Bundles are read-only: "a *git push* into a bundle is not supported".

**Shortfalls.** No forge metadata, no LFS, and mirror updates are destructive overwrites.

### 1.6 GitHub Archive Program

**The 2020 snapshot.** On 2020-02-02 GitHub took one snapshot of the HEAD of each qualifying repo's default branch ([Arctic Vault page](https://archiveprogram.github.com/arctic-vault/)).
- Qualifying repos had commits between 2019-11-13 and 2020-02-02, or 1+ star and commits in the prior year, or 250+ stars.
- Binaries over 100 KB were stripped except in repos with 250+ stars.
- The 21 TB were written to 186 reels of Piql film ([Arctic Vault page](https://archiveprogram.github.com/arctic-vault/)).

**Tiers and partners.** The program describes "hot, warm, and cold" tiers. Partners include the Internet Archive, Software Heritage, GH Archive, and GHTorrent, plus film copies at the Bodleian Library, Bibliotheca Alexandrina, and Stanford ([approach](https://archiveprogram.github.com/approach/), [partners](https://archiveprogram.github.com/partners/)).

**Shortfalls.**
- It was a one-time, HEAD-only cold snapshot with no history.
- There is no on-demand saving and no practical retrieval path.
- The pages do not describe any later Arctic snapshot (not verified either way).

### 1.7 Archive Team / Internet Archive

- **Internet Archive:** "commenced its ongoing archive of GitHub public repositories on April 13, 2020", storing WARCs, with the aim of making repos available through `git clone` ([partners](https://archiveprogram.github.com/partners/)). No status update on the `git clone` goal was found.
- **Archive Team (as described on its own wiki):** a community-maintained wiki, so **[lower confidence]** ([AT wiki: GitHub](https://wiki.archiveteam.org/index.php?title=GitHub)).
  - The DPoS project `github-grab` archived the web UI: issues, PR conversation, commits and checks tabs, releases, wikis, and downloads. The wiki marks this description as "out-of-date".
  - The **current** activity is a bot (`#gitgud` on hackint, commands `!scn` and `!gitscn`) that submits repos to Software Heritage Save Code Now. It also grabs release and issue links, up to 500 repos per org or user URL.
  - The repo is [ArchiveTeam/github-grab](https://github.com/ArchiveTeam/github-grab). Its last push was 2025-08-05 **[observed]**.
- **Shortfalls:** WARCs are web captures rather than restorable forge data. The data collection is marked private on IA (wiki infobox). Participation needs Archive Team's Warrior or Docker and its rules ([github-grab README](https://github.com/ArchiveTeam/github-grab)).

### 1.8 Other tools

**python-github-backup (`github-backup`, josegonzalez)** has the widest GitHub metadata capture of any tool found ([README](https://github.com/josegonzalez/python-github-backup)).
- **What it saves:** issues, comments, events, and timeline; PRs, review comments, reviews, and commits; labels, milestones, hooks, and security advisories. It also covers **discussions** through GraphQL, because "the REST API does not expose discussions". Beyond those it saves wikis, releases, assets, attachments, gists, starred repos, and forks.
- **Clones:** `--lfs` is supported. `--bare` actually runs `clone --mirror`.
- **Incremental runs** use per-repo checkpoints, with a GraphQL cross-reference check because "being referenced from elsewhere does not change an issue's `updated_at`".
- **Restore:** none. "This tool creates backups only, there is no inbuilt restore command". Recreating issues through the API cannot keep the original numbers, timestamps, or authors.
- **Defaults:** `--all` excludes forks, LFS, attachments, and gists.
- **Scope:** GitHub only.

**gickup** "clone/mirror[s] repositories from one host to another" ([README](https://github.com/cooperspencer/gickup)).
- **Sources:** GitHub, GitLab, Codeberg, Gitea, Gogs, Bitbucket, OneDev, Sourcehut, and others.
- **Destinations:** forges, local disk, **S3**, and **Radicle**.
- **Options:** `issues: true` ("works only locally"), wiki, LFS, `bare`/`mirror`, `zip`, `keep: N` (rotating backups), and cron ([conf.example.yml](https://github.com/cooperspencer/gickup/blob/main/conf.example.yml)).
- **Shortfalls:** issue capture is local-only, and there is no PR, review, or release-asset capture in the example config.

**github-backup (Joey Hess)** "backs up everything GitHub publishes about the repository, including branches, tags, other forks, issues, comments, wikis, milestones, pull requests, watchers, and stars" ([site](https://github-backup.branchable.com/)). It was withdrawn in December 2020 and "needs a new maintainer". Joey's reasons were that Software Heritage already covers GitHub git data and that users showed little interest even after youtube-dl ([blog](https://joeyh.name/blog/entry/Withrawing_github-backup/)).

**GitHub's own exports.**
- **User migration API:** `POST /user/migrations` produces a tar.gz with JSON for issues, PRs, reviews, comments, releases, milestones, and more, plus git data and attachments. "These endpoints are only available to authenticated account owners" ([REST migrations/users](https://docs.github.com/en/rest/migrations/users)). It is not usable for third-party repos.
- **DMCA §1201 cases:** GitHub "aim[s] to let owners export issues, pull requests, and other data" when it disables content ([DMCA policy §C](https://docs.github.com/en/site-policy/content-removal-policies/dmca-takedown-policy#c-what-about-circumvention-claims)).

**GitLab project export** includes issues, MRs (latest diff only), comments, wiki, uploads, LFS, releases, and archived pipelines. It requires the Maintainer or Owner role, and the docs say: "You should not use project export files to back up your data" ([GitLab docs](https://docs.gitlab.com/user/project/settings/import_export/)).

**GitLab pull mirroring** is Premium/Ultimate only.
- It pulls automatically 30 minutes after the previous pull, and that interval "cannot be disabled". It stops after 14 consecutive failures.
- By default it **does not** delete branches or tags that were deleted upstream, and it stops updating branches that have diverged ([GitLab docs](https://docs.gitlab.com/user/project/repository/mirror/pull/)). That non-destructive default is unusual.

### 1.9 Comparison table

| Tool | Captures | Model | Main weaknesses for at-risk archiving |
|---|---|---|---|
| Software Heritage | Git objects and per-visit snapshots of refs (incl. `refs/pull/*/head`, not `/merge`). No LFS, issues, PRs, or releases. | Central service (Inria, French law). Save Code Now API and webhooks. Vault for retrieval. | Coverage gaps for small or new repos (4/5 DMCA'd samples missing). Code only. Async retrieval. Has its own takedown process. |
| Radicle | Git repos plus COBs (issues, patches) native to Radicle. | P2P. Self-hosted nodes with seeding policies, plus public seeds. | Only works for repos published to Radicle. No GitHub metadata import. No documented way to seed third-party GitHub repos. |
| ghorg | Mirror clones, wikis, submodules, LFS (opt-in). | Self-hosted CLI. Bulk by org or user. Cron. | No issues, PRs, or releases. Default re-run overwrites. `--prune` deletes clones of removed repos. |
| Forgejo/Gitea pull mirror | Branches and tags (mirror clone, `fetch --prune --tags`). LFS opt-in. | Self-hosted forge. | No metadata on mirrors. Prune on by default. Force-pushes propagate. |
| Forgejo/Gitea migration / `dump-repo` | Git, wiki, issues, PRs, reviews, comments, labels, milestones, releases, assets, LFS. | Self-hosted forge. One-shot. `dump-repo` writes a portable dump. | Not continuous. Re-importing is a new snapshot. |
| `git clone --mirror` | Every advertised ref (heads, tags, notes, `refs/pull/*` on GitHub). | Local. | No LFS, submodule targets, wiki, metadata, or fork branches. Updates overwrite. |
| `git bundle` | Objects for chosen refs, as one file. Supports incremental bundles. | Local file. | Read-only. Same gaps as `--mirror`. |
| GitHub Archive Program | Default-branch HEAD of qualifying repos, 2020-02-02. | GitHub and partners. Cold storage on film. | One-time. No history. No on-demand use. Not retrievable in practice. |
| Archive Team / IA | Web UI WARCs (historically). Now submits to SWH. | Volunteer distributed crawling and a central IA collection. | WARCs are not restorable forge data. The collection is private. Effort is irregular. |
| python-github-backup | Git (mirror), LFS, wiki, issues, events, timeline, PRs, reviews, review comments, discussions, releases, assets, attachments, gists, forks. | Self-hosted CLI. JSON plus clones. Incremental. | GitHub only. No restore. Many items are off under `--all`. Heavy on API usage. |
| gickup | Mirror or bare clones, wiki, LFS, issues (local). | Self-hosted daemon (cron). Many sources. Destinations include forges, S3, Radicle, and local. | Thin metadata (no PRs or releases). Mirror semantics. |
| github-backup (Joey Hess) | Branches, forks, issues, comments, wiki, milestones, PRs. | Self-hosted CLI. | Withdrawn and unmaintained since 2020. |
| GitHub user migration API | Full export with metadata. | GitHub, owner-only. | Only for repos you own. |
| GitLab project export | Git, wiki, issues, MRs, LFS, releases, uploads. | GitLab, Maintainer or Owner only. | Not for third parties. GitLab says not to use it for backups. |

---

## 2. What a "complete" archive is

### 2.1 What `git clone --mirror` takes and misses

`--mirror` takes every ref the server advertises ([git-clone](https://git-scm.com/docs/git-clone)). What that covers depends on the forge.

| Part | In `--mirror`? | Notes and source |
|---|---|---|
| Branches and tags | Yes | [git-clone](https://git-scm.com/docs/git-clone) |
| Notes (`refs/notes/*`) | Yes, if the server advertises them | "maps all refs (including … notes etc.)" ([git-clone](https://git-scm.com/docs/git-clone)) |
| GitHub `refs/pull/N/head` and `refs/pull/N/merge` | Yes | `cli/cli` advertised 5,356 `refs/pull/N/head` and 167 `refs/pull/N/merge` **[observed]**. "The remote `refs/pull/` namespace is *read-only*" ([GitHub docs](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/reviewing-changes-in-pull-requests/checking-out-pull-requests-locally)). The `/merge` refs are GitHub-made test merges that GitHub regenerates, and SWH drops them ([swh-loader-git](https://gitlab.softwareheritage.org/swh/devel/swh-loader-git/-/blob/da7c9eba3c737541b7a1834d0391b2f9d935d453/swh/loader/git/utils.py#L95-112)). |
| GitLab `refs/merge-requests/N/{head,merge,train}`, `refs/pipelines/*`, `refs/environments/*`, `refs/workloads/*` | Yes (advertised) | Seen on `gitlab-org/cli` **[observed]** |
| LFS objects | **No** | Only pointer files are in git. Use `git lfs fetch --all`, "primarily for backup and migration purposes" ([git-lfs-fetch](https://github.com/git-lfs/git-lfs/blob/main/docs/man/git-lfs-fetch.adoc)). |
| Submodule contents | **No** | `--recurse-submodules` is ignored with `--mirror` ([git-clone](https://git-scm.com/docs/git-clone)). Each submodule is a separate repo (URL in `.gitmodules`, a commit pinned as a gitlink) ([gitsubmodules](https://git-scm.com/docs/gitsubmodules)) and must be archived as its own repo. |
| Wiki | **No** | A separate git repo at `https://github.com/OWNER/REPO.wiki.git` ([GitHub docs](https://docs.github.com/en/communities/documenting-your-project-with-wikis/adding-or-editing-wiki-pages)) |
| Forks' own branches | **No** | Forks are separate repos. Only fork commits that were submitted as PRs appear, through `refs/pull/*`. |
| Issues, PRs as conversations, reviews, releases, assets, discussions, projects | **No** | Not in git. Use the APIs in 2.2. |

### 2.2 APIs for non-git data

**GitHub REST** (all list endpoints take `per_page` up to 100):

| Data | Endpoint | Source |
|---|---|---|
| Issues (and PRs as issues) | `GET /repos/{o}/{r}/issues?state=all&since=…`. "every pull request [is] an issue". PRs carry a `pull_request` key. | [issues](https://docs.github.com/en/rest/issues/issues#list-repository-issues) |
| Issue comments | `GET /repos/{o}/{r}/issues/comments` (repo-wide) | [issue comments](https://docs.github.com/en/rest/issues/comments) |
| Issue events and timeline | `GET /repos/{o}/{r}/issues/{n}/events` and `GET /repos/{o}/{r}/issues/{n}/timeline` | [events](https://docs.github.com/en/rest/issues/events), [timeline](https://docs.github.com/en/rest/issues/timeline) |
| Pull requests | `GET /repos/{o}/{r}/pulls?state=all` | [pulls](https://docs.github.com/en/rest/pulls/pulls) |
| PR review (diff) comments | `GET /repos/{o}/{r}/pulls/comments` (repo-wide) | [review comments](https://docs.github.com/en/rest/pulls/comments) |
| PR reviews | `GET /repos/{o}/{r}/pulls/{n}/reviews` | [reviews](https://docs.github.com/en/rest/pulls/reviews) |
| Releases | `GET /repos/{o}/{r}/releases` | [releases](https://docs.github.com/en/rest/releases/releases) |
| Release assets (binary) | `GET /repos/{o}/{r}/releases/assets/{id}` with `Accept: application/octet-stream`. Expect 200 or 302. Up to 1,000 assets per release, each under 2 GiB, no total size limit. | [assets](https://docs.github.com/en/rest/releases/assets), [about releases](https://docs.github.com/en/repositories/releasing-projects-on-github/about-releases) |
| Forks | `GET /repos/{o}/{r}/forks` (sort `newest`, `oldest`, `stargazers`, `watchers`). The docs do not say whether forks of forks are included. Assume recursion is needed (unverified). | [forks](https://docs.github.com/en/rest/repos/forks) |
| Discussions | GraphQL only (`Repository.discussions`). python-github-backup confirms the REST API does not expose them. | [GraphQL discussions guide](https://docs.github.com/en/graphql/guides/using-the-graphql-api-for-discussions), [pgb README](https://github.com/josegonzalez/python-github-backup) |
| Owner-only full export | `POST /user/migrations` | [migrations](https://docs.github.com/en/rest/migrations/users) |

GraphQL can batch most of these (e.g. `PullRequest.reviewThreads`) at 1 point per 100 nodes ([GraphQL limits](https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api)).

Issue and PR attachments (user-uploaded images and files) are separate URLs. Fine-grained tokens cannot fetch document attachments from private repos ([pgb README](https://github.com/josegonzalez/python-github-backup)) **[lower confidence; third-party tool docs]**.

**GitLab REST:**
- [issues](https://docs.gitlab.com/api/issues/)
- [merge requests](https://docs.gitlab.com/api/merge_requests/)
- [notes](https://docs.gitlab.com/api/notes/) and [discussions](https://docs.gitlab.com/api/discussions/) (comments and threads)
- [releases](https://docs.gitlab.com/api/releases/)
- [wikis](https://docs.gitlab.com/api/wikis/)
- [project export](https://docs.gitlab.com/api/project_import_export/), Maintainer or higher only

### 2.3 The fork network's shared object store

- **Shared storage:** "Repositories in the network share Git data", "Commits pushed to any repository in a network can be accessible from other repositories in that network", and "Git data from any repository in a network may be accessed from any repository in the same network … even after a fork is deleted" ([GitHub docs](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/working-with-forks/about-permissions-and-visibility-of-forks)).
- **Parent deleted:** if a public parent is deleted, "An active public fork becomes the new upstream repository for the network" ([GitHub docs](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/working-with-forks/what-happens-to-forks-when-a-repository-is-deleted-or-changes-visibility)).
- **Parent made private:** if a public parent is made private, "Its public forks stay public in a separate network" (same source).
- **Commits survive with the network:** "Commits in a pull request are available in a repository even before the pull request is merged", and PR changes survive a deleted fork ([GitHub docs](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/reviewing-changes-in-pull-requests/checking-out-pull-requests-locally)).
- **Implications:**
  - Fork branches are not advertised in the parent's ref list unless they became PRs (`refs/pull/*`).
  - The docs say network objects are reachable from other repos but do not document fetching an unadvertised SHA. Whether a client can `git fetch <sha>` for a fork-only commit was not verified.
  - A DMCA notice that names the whole network can disable all of it ([github/dmca README](https://github.com/github/dmca/blob/master/README.md)).

---

## 3. Detecting takedowns and risk

### 3.1 Signals per state (GitHub)

| State | REST `GET /repos/{o}/{r}` | GraphQL `repository(owner,name)` | Git over HTTPS | Web |
|---|---|---|---|---|
| **DMCA-disabled** | **451** `{"message":"Repository access blocked","block":{"reason":"dmca","created_at":"…","html_url":"https://github.com/github/dmca/blob/master/…md"}}` **[observed]**, same with or without auth | `NOT_FOUND` "Could not resolve to a Repository" **[observed]** | `remote: Repository unavailable due to DMCA takedown.` / `remote: See the takedown notice for more details:` / `remote:   <notice URL>` then `fatal: … returned error: 403` **[observed]** | 451 **[observed]** |
| **Deleted or private** | 404 `{"message":"Not Found"}` **[observed]** | `NOT_FOUND` **[observed]** | `remote: Repository not found.` **[observed]** | 404 |
| **Renamed or transferred** | 301 "Moved permanently" ([docs](https://docs.github.com/en/rest/repos/repos#get-a-repository)) | n/a | git operations to the old URL "will continue to function". Redirects break if a new repo takes the old name ([rename docs](https://docs.github.com/en/repositories/creating-and-managing-repositories/renaming-a-repository)). | Redirect |
| **Suspended or hidden user** | Not documented. Community reports say repos return 404 to others **[lower confidence]**. GitHub lists "Hiding a user account or organization from public view" and "Suspending a user account or organization" as enforcement actions ([Community Guidelines](https://docs.github.com/en/site-policy/github-terms/github-community-guidelines)). | Not verified | Not verified | Not verified |

**Notes on these signals.**
- **451 is documented only in the 2016 changelog.** That changelog introduced 451 "for repositories and gists" in place of 403 ([changelog](https://developer.github.com/changes/2016-03-17-the-451-status-code-is-now-supported)). Today's REST reference does not list it ([Get a repository](https://docs.github.com/en/rest/repos/repos#get-a-repository)).
- **Other `block.reason` values are unknown.** Values other than `dmca` (e.g. government takedowns, AUP actions) were not observed and are not documented. Government notices are published separately in [`github/gov-takedowns`](https://github.com/github/gov-takedowns).
- **The repo ID is stable across renames.** `GET /repositories/{id}` resolved `212613049` → `cli/cli` **[observed]**. This endpoint was not found in the REST reference, so treat it as **undocumented**.
- **The owner endpoint can disambiguate.** `GET /users/{login}` still returned 200 for the owner of a deleted repo **[observed]**. Telling a deleted user apart from a suspended or hidden one from the outside is not documented.

### 3.2 The `github/dmca` repo as a watch feed

**Location and layout.** [`github.com/github/dmca`](https://github.com/github/dmca), default branch `master`, contains notices, counter-notices, and retractions as Markdown at `YYYY/MM/YYYY-MM-DD-<slug>.md` **[observed]**.
- **Suffixes:** variants carry suffixes such as `-2`, `-counternotice`, `-retraction`, and `-reversal` (e.g. `2026/10/2026-10-13-ford-retraction.md`) **[observed]**.
- **Attachments:** PDF attachments sit alongside the notices (e.g. `2020-10-23-RIAA-court-order.pdf`).
- **Data files:** `data/<year>/*-COMPILED-DATA.csv` (columns: Filename, Year, Month, Date, Description, Type of Notice, Number of Repos Affected) exists only for 2015–2021 **[observed]**.

**Notice format.** Notices are free-text Markdown built from GitHub's form, with bold question headers and the submitter's answers ([example](https://github.com/github/dmca/blob/master/2026/11/2026-11-02-typora.md)).
- **Machine-readable fields:** none. Infringing URLs appear as plain `https://github.com/...` lines, and the answer to "entire repository is infringing" vs. "specific files" is plain text.
- **Redactions:** URLs GitHub found not actionable are replaced with `[invalid]`. Private information is replaced with `[private]` (before March 2021, `[private]` was used for both).
- **Annotations:** the header can note that owners were given time to make changes, or that a whole fork network was processed ([README](https://github.com/github/dmca/blob/master/README.md)).

**Timing (decision-relevant).** For five full-repo notices from October 2026, the commit adding the notice came after the repo's `block.created_at` **[observed]**:

| Notice | `block.created_at` | Notice commit |
|---|---|---|
| Veritone | 2026-10-06T20:55:55Z | 20:56:50Z |
| bephax | 2026-10-01T14:08:02Z | 14:08:38Z |
| nintendo | 2026-10-05T21:25:19Z | 21:33:09Z |
| epic-games | 2026-10-05T15:58:54Z | 15:59:32Z |
| techno-speech | 2026-10-01T17:41:53Z | 17:42:38Z |

This is a small sample. For notices about *specific files*, GitHub's policy gives the owner "approximately 1 business day" to make changes before disabling ([policy §A](https://docs.github.com/en/site-policy/content-removal-policies/dmca-takedown-policy#a-how-does-this-actually-work)). Whether the notice is published before that window opens was not established.

**Feed mechanics.**
- The Atom feed `https://github.com/github/dmca/commits/master.atom` returns 200 `application/atom+xml` **[observed]**.
- Polling the commits API or running `git fetch` on the repo also works.
- Activity is bursty. The last 100 commits fell on 6 days, from 2 to 36 commits per day **[observed]**.

**What a feed can and cannot do.**
- *Too late for named repos:* by the time a notice is committed, the repos it names are already blocked.
- *Fork coverage:* GitHub disables forks only when they are named, or when the notice covers the whole network ([policy §B](https://docs.github.com/en/site-policy/content-removal-policies/dmca-takedown-policy#b-what-about-forks-or-whats-a-fork)). Unnamed forks may still be fetchable after the notice appears.
- *Restoration tracking:* the same feed announces counter-notices and retractions, which signal possible restoration (10–14 business days, §6).
- *A preemptive watch list needs other signals.* The sources do not name any such signals.

---

## 4. Operational constraints

### 4.1 GitHub

**REST primary limits** ([REST rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api)):

| Client | Limit |
|---|---|
| Unauthenticated (per IP) | 60/hour |
| Personal access token or user | 5,000/hour |
| GitHub App installation | 5,000/hour minimum, up to 12,500/hour (15,000 for Enterprise Cloud) |
| `GITHUB_TOKEN` in Actions | 1,000/hour per repo |

**REST secondary limits** (same source):
- 100 concurrent requests, shared with GraphQL.
- 900 points per minute per REST endpoint, where GET costs 1 and writes cost 5.
- 90 s of CPU per 60 s of real time.
- Content creation is "generally" 80/minute and 500/hour.
- A secondary limit returns 403 or 429. Honour `retry-after`, otherwise wait at least 1 minute, then back off exponentially.
- "Continuing to make requests while you are rate limited may result in the banning of your integration."

**Git LFS API** is a separate bucket: 300/minute unauthenticated and 3,000/minute authenticated, with 100 objects per batch (same source).

**GraphQL** ([GraphQL limits](https://docs.github.com/en/graphql/overview/rate-limits-and-query-limits-for-the-graphql-api)):
- 5,000 points/hour per user (10,000 for Enterprise Cloud apps).
- Cost is the number of requests needed divided by 100, with a minimum of 1.
- `first`/`last` must be between 1 and 100, with at most 500,000 nodes per call.
- Calls time out after 10 s, and a timeout costs extra points.
- The secondary limit is 2,000 points/minute.

**Git clone and fetch.**
- GitHub publishes no numeric limit for authenticated clones.
- Since 2025-05-08, unauthenticated HTTPS clones, anonymous REST calls, and `raw.githubusercontent.com` have new rate limits, without published numbers. GitHub cites "an increase in scraping activity" ([changelog](https://github.blog/changelog/2025-05-08-updated-rate-limits-for-unauthenticated-requests/)).
- Since 2026-10-19, GitHub "may temporarily require authentication for some HTTPS clones and fetches of public repositories" during unusual traffic. Authenticated HTTPS and SSH are unaffected ([changelog](https://github.blog/changelog/2026-10-19-some-git-over-https-may-temporarily-require-authentication)).
- A GitHub Support reply in a community thread says git operations have "dynamic limits" ([discussion #44515](https://github.com/orgs/community/discussions/44515)) **[lower confidence]**.

**LFS bandwidth** ([LFS billing](https://docs.github.com/en/repositories/working-with-files/managing-large-files/about-storage-and-bandwidth-usage)):
- Free, Pro, and Free-org accounts get 10 GiB of storage and 10 GiB of bandwidth per month. Team and Enterprise Cloud get 250 GiB.
- "the bandwidth you use is included in the repository owner's bandwidth usage".
- If the owner has no payment method and runs out of bandwidth, LFS is disabled for the account until the next month.
- Usage in forks counts against the parent repo.
- Maximum LFS file sizes are 2/2/4/5 GB by plan ([about LFS](https://docs.github.com/en/repositories/working-with-files/managing-large-files/about-git-large-file-storage)).

**Terms** ([AUP](https://docs.github.com/en/site-policy/acceptable-use-policies/github-acceptable-use-policies), [ToS](https://docs.github.com/en/site-policy/github-terms/github-terms-of-service); text checked against [github/site-policy @ b9578b5](https://github.com/github/site-policy)):
- **AUP §7:** "Archivists may use public information from the Service for archival purposes." Scraping is defined as automated extraction and "does not refer to the collection of information through our API." Using the information for spam or for selling personal information is prohibited.
- **AUP §4:** prohibits "using our servers for any form of excessive automated bulk activity, to place undue burden on our servers through automated means".
- **AUP §6:** "You will not reproduce, duplicate, copy, sell, resell or exploit any portion of the Service … without our express written permission."
- **AUP §9:** for bandwidth "significantly excessive in relation to other users of similar features", GitHub may "suspend your Account, throttle your file hosting, or otherwise limit your activity".
- **ToS §D.5:** public repos grant other users a licence to "use, display, perform and reproduce (by forking) Your Content through the Service".
- **ToS §D.8:** "these Terms do not restrict lawful access to or use of the contents of public repositories by third parties".
- **ToS §H:** "Abuse or excessively frequent requests … may result in the temporary or permanent suspension of your Account's access to the API"; "You may not share API tokens to exceed GitHub's rate limitations."

### 4.2 GitLab.com

Source: [GitLab.com rate limits](https://docs.gitlab.com/user/gitlab_com/rate_limits/). The current limits are below. Proposed per-plan limits exist, and "the lower limit applies".

| Traffic | Limit |
|---|---|
| Authenticated API | 2,000/minute per user |
| Unauthenticated | 500/minute per IP (proposed: 60/hour) |
| All traffic from one IP | 2,000/minute |
| Git HTTPS | 10,000/minute authenticated, 15,000/minute unauthenticated per IP |
| Git SSH | 600/minute per user, project, and command |
| `/api/v4/projects` listing | 2,000 per 10 minutes authenticated, 400 unauthenticated |
| LFS | No separate limit. On Free, Git HTTPS, LFS, and dependency proxy share 100/minute. |

Also:
- Proposed Free-plan API limits are 5,000/hour sustained and 100/minute burst.
- Gitaly concurrency is capped at 60 HTTP and 30 SSH clones ([GitLab.com settings](https://docs.gitlab.com/user/gitlab_com/)).
- No GitLab.com ToS clause was checked for this report.

### 4.3 Codeberg

- **API rate limits:** no numbers published in the FAQ or Terms of Use (not found).
- **Mirrors:** "Mirrors that pull content from other code hosting services were problematic for Codeberg" because of traffic and disk use. Manual multi-remote `git push --mirror` is suggested instead ([FAQ](https://docs.codeberg.org/getting-started/faq/)).
- **Storage:** request resources first for more than 750 MiB of git storage or 1.5 GiB of packages, LFS, and attachments ([FAQ](https://docs.codeberg.org/getting-started/faq/)).
- **Terms of Use** ([ToU](https://codeberg.org/Codeberg/org/src/branch/main/TermsOfUse.md)):
  - Only FLOSS, open hardware, or free-culture content is allowed (§2(1)1).
  - "You must only share content on Codeberg which you have the explicit right … to share" (§2(1)3).
  - "Forks, migrations and testing repos are considered as inactive when they don't contain unique contributions and are inactive for more than a month … might be removed after notifying maintainers and providing a 90 days period" (§2(1)4).
  - Content "deemed illegal in Germany … will be taken offline" (§2(4)).
  - Excessive traffic or storage normally gets a warning first (§3(5)).

### 4.4 Bitbucket Cloud

Source: [API request limits](https://support.atlassian.com/bitbucket-cloud/docs/api-request-limits/).
- Anonymous API: 60/hour per IP.
- Git operations over HTTPS and SSH: 60,000/hour.
- Raw files: 5,000/hour.
- Archive downloads: 1,500 files/hour.
- Repository data endpoints: 1,000/hour, scaled up to 10,000/hour for large paid workspaces.
- All limits use a one-hour rolling window.
- No LFS limit is listed.

---

## 5. Storage and distribution options

| Option | What it is (sourced) | What it means for this tool |
|---|---|---|
| **Plain bare or mirror repos** | `git clone --mirror`. Updates overwrite all refs ([git-clone](https://git-scm.com/docs/git-clone)). | Simplest, fully servable, and can be fetched incrementally. Overwrite semantics mean preserving history across force-pushes or deletions takes extra handling (e.g. snapshots, a non-pruning fetch, ref namespacing). Needs a filesystem or git server. |
| **Bundles** | Single-file pack plus ref header. Full (`--all`) or incremental (`old..new`, needs prerequisites). Verifiable. Clonable. Read-only ([git-bundle](https://git-scm.com/docs/git-bundle)). Bundle URIs let a client bootstrap from static bundle files listed in a config-format bundle list, HTTP(S) only so far ([bundle-uri](https://git-scm.com/docs/bundle-uri)). | Self-contained, immutable artifacts. Easy to hash, sign, and upload to dumb storage (S3, IA, torrents, IPFS). Incremental chains need every prerequisite kept. Not "live" without a re-clone. |
| **IPFS** | The official how-to: `git clone --mirror`, `git update-server-info`, optionally `git unpack-objects` (lets IPFS deduplicate objects across versions), `ipfs add -r`, then clone over a gateway as dumb HTTP. "This provides no security guarantees, as a man-in-the-middle attack could ship you bad code" applies to the Go-import scenario ([host-git-repo](https://docs.ipfs.tech/how-to/host-git-repo/)). Content survives only while some node pins it. Unpinned data is garbage-collected ([persistence](https://docs.ipfs.tech/concepts/persistence/)). | Content-addressed and censorship-resistant distribution, but each update has a new CID, so a mutable name (IPNS, not covered by the how-to) or a manifest is needed. Durability depends on who pins. |
| **BitTorrent** | BEP 46 "enables torrents to update based on data stored in the BitTorrent DHT" using `magnet:?xs=urn:btpk:<pubkey>`. Status: **Draft** ([BEP 46](https://www.bittorrent.org/beps/bep_0046.html)). | Good for distributing large immutable bundles to many holders. Updating needs BEP 46 (draft, so client support varies, not verified) or new torrents. Durability depends on seeders. |
| **S3-compatible** | `awslabs/git-remote-s3` stores refs as `<prefix>/<ref>/<sha>.bundle`, uses per-ref locking for concurrent pushes, and supports an LFS custom transfer ([README](https://github.com/awslabs/git-remote-s3)). gickup supports S3 as a destination with `zip` and storage-class options ([conf.example.yml](https://github.com/cooperspencer/gickup/blob/main/conf.example.yml)). | Cheap, durable, provider-neutral (MinIO, Garage, B2, and others are S3-compatible, not verified per provider). Not git-servable without a helper or bundle-URI. Takedown exposure moves to the bucket provider. |
| **Pushing to multiple forges as mirrors** | `git push --mirror` ([git-push](https://git-scm.com/docs/git-push)). Forgejo push mirrors force-push, and LFS over SSH is not mirrored ([Forgejo docs](https://forgejo.org/docs/latest/user/repo-mirror/)). Pushing to `refs/pull/*` on GitHub is rejected ("deny updating a hidden ref", [GitHub docs](https://docs.github.com/en/pull-requests/collaborating-with-pull-requests/reviewing-changes-in-pull-requests/checking-out-pull-requests-locally)). | Gives public, browsable redundancy, but each forge applies its own takedown process (GitHub re-posting rule, Codeberg ToU §2(1)3, §2(4), inactive-migration cleanup §2(1)4). Forge-reserved ref namespaces (`refs/pull/*`, `refs/merge-requests/*`) may need renaming. Mirror push is destructive on the target. |
| **Software Heritage as a sink** | Save Code Now plus Vault ([§1.1](#11-software-heritage-swh)) | A free, independent extra copy of the *code*, but under SWH's control and takedown policy. |

---

## 6. Legal landscape (factual, not legal advice)

### 6.1 17 U.S.C. §512 (safe harbors)

Source: [17 U.S.C. §512, LII](https://www.law.cornell.edu/uscode/text/17/512).

- **§512(c)(1), storage at a user's direction.** A service provider "shall not be liable for monetary relief" (and, with exceptions, injunctive relief) for infringement "by reason of the storage at the direction of a user" if it:
  - (A) lacks actual knowledge or "red flag" awareness, or on gaining it "acts expeditiously to remove, or disable access to, the material";
  - (B) gets no directly attributable financial benefit where it has the right and ability to control; and
  - (C) "upon notification of claimed infringement as described in paragraph (3), responds expeditiously to remove, or disable access to" the material.
- **§512(c)(2):** the safe harbor requires a designated agent.
- **§512(c)(3):** sets the required elements of a notice.
- **§512(g)(1):** no liability for good-faith takedown.
- **§512(g)(2):** for material taken down on a §512(c) notice, the provider keeps that protection only if it:
  - (A) notifies the subscriber;
  - (B) forwards any counter-notice to the claimant, saying it will restore in 10 business days; and
  - (C) "replaces the removed material and ceases disabling access to it not less than 10, nor more than 14, business days following receipt of the counter notice, unless" the claimant reports filing "an action seeking a court order".
- **§512(g)(3):** a counter-notice must include a signature, identification of the material, a statement "under penalty of perjury" of good-faith belief of "mistake or misidentification", and consent to federal jurisdiction and service of process.
- **§512(f):** knowing material misrepresentation in a notice or counter-notice creates liability for damages.
- **§512(h):** allows subpoenas to identify an alleged infringer.
- **§512(i)(1)(A):** the safe harbors require a policy for terminating "repeat infringers".

**GitHub's implementation** ([DMCA policy](https://docs.github.com/en/site-policy/content-removal-policies/dmca-takedown-policy)):
- About 1 business day for the owner to make changes when the notice targets specific content.
- Whole-repo claims are disabled directly.
- Two weeks of claimant silence after changes counts as an implied retraction.
- Counter-notices lead to restoration after 10–14 days unless a lawsuit is filed.
- Forks are not auto-disabled.
- Notices are published in `github/dmca`, and a link to the notice is posted where content was removed.
- Repeat infringers' accounts may be terminated (§F).

### 6.2 17 U.S.C. §1201 (anti-circumvention)

Source: [17 U.S.C. §1201, LII](https://www.law.cornell.edu/uscode/text/17/1201).

- **§1201(a)(1)(A):** "No person shall circumvent a technological measure that effectively controls access to a work protected under this title."
- **§1201(a)(2):** bars trafficking in any technology that "(A) is primarily designed or produced for the purpose of circumventing a technological measure that effectively controls access…", "(B) has only limited commercially significant purpose or use other than to circumvent…", or "(C) is marketed … for use in circumventing…".
- **§1201(b)(1):** a parallel trafficking ban for measures that protect the *rights* of a copyright owner, as opposed to access.
- **How GitHub handles §1201 claims** ([policy §C](https://docs.github.com/en/site-policy/content-removal-policies/dmca-takedown-policy#c-what-about-circumvention-claims)):
  - It requires a detailed technical description and reviews claims with technical and legal experts.
  - If it cannot determine validity, it leaves the content up.
  - It contacts the owner first.
  - It aims to let owners export issues and PRs.
  - It offers a Developer Defense Fund referral.
- The text of §512(g) refers to material "claimed to be infringing". Whether and how the §512 counter-notice mechanism applies to a pure §1201 claim is not stated in the sources reviewed here.

### 6.3 Case study: youtube-dl (2020)

- **2020-10-23, takedown.** The RIAA notice ([`2020/10/2020-10-23-RIAA.md`](https://github.com/github/dmca/blob/master/2020/10/2020-10-23-RIAA.md)):
  - It named `ytdl-org/youtube-dl`, its gh-pages, and about 18 forks as a "representative sample", stating "a good faith belief that most of the youtube-dl forks are infringing to the same extent as the parent repository."
  - It asserted the code's purpose was to "circumvent the technological protection measures used by authorized streaming services such as YouTube".
  - It cited test or example references to three commercial music videos.
  - It claimed a violation of "17 USC §§1201(a)(2) and 1201(b)(1)", attaching a Hamburg Regional Court decision about YouTube's "rolling cipher" ([court order PDF](https://github.com/github/dmca/blob/master/2020/10/2020-10-23-RIAA-court-order.pdf)).
- **2020-11-16, reinstatement.** The reversal ([`2020/11/2020-11-16-RIAA-reversal.md`](https://github.com/github/dmca/blob/master/2020/11/2020-11-16-RIAA-reversal.md)) says:
  - "The parent repository made changes in response to the takedown notice, and based on additional information" (linked as [`…-reversal-effletter.pdf`](https://github.com/github/dmca/blob/master/2020/11/2020-11-16-RIAA-reversal-effletter.pdf)), "GitHub has determined that the notice does not meet the requirements of our DMCA Takedown Policy".
  - GitHub reinstated the parent repo and "will reinstate forked repositories that make similar changes".
- **GitHub's blog post** ([2020-11-16, updated 2021-08-24](https://github.blog/news-insights/policy-news-and-insights/standing-up-for-developers-youtube-dl-is-back/)) says:
  - GitHub concluded "the allegations did not establish a violation of the law".
  - Going forward, "Every single credible 1201 takedown claim will be reviewed by technical experts", and where ambiguous, "we will err on the side of the developer".
  - GitHub would "establish and donate $1M to a developer defense fund".
- **Facts relevant to archiving:**
  - The parent and a sample of forks were disabled for about 3.5 weeks.
  - Forks were restored only if they made similar changes.
  - Joey Hess, author of a GitHub backup tool, cited the episode when withdrawing github-backup ([blog](https://joeyh.name/blog/entry/Withrawing_github-backup/)).

---

## 7. Open questions for design

These are for the human to decide. None are answered here.

1. **Threat model and scope.** Which events must the archive survive? Options include DMCA disable (possibly temporary), owner deletion, account suspension or hiding, force-push history rewrites, forge shutdown, and the archive host's own takedowns. Is "temporarily unavailable, then restored" a case the tool must model?
2. **Selection.** How do repos get into the archive? Options include a hand-picked list, stars or dependencies of a seed set, org or user sweeps, a `github/dmca`-driven fork rescue, or community submission. Given that the feed comes after the takedown (§3.2), what is the feed for?
3. **Definition of "complete".** Which of these are in scope: `refs/pull/*/head`, `refs/pull/*/merge`, notes, LFS, submodule targets (recursively?), wiki, issues, PRs, reviews, discussions, releases, release assets, attachments, forks (all, top-N, or only those with unique commits)?
4. **History preservation semantics.** Should the archive keep refs that upstream force-pushed or deleted? If so, how should they be named or snapshotted? Should it be append-only? Should mirrors prune?
5. **LFS ethics and cost.** Fetching LFS spends the owner's GitHub bandwidth quota and can disable their LFS for the month (§4.1). Should LFS fetching be on, off, opt-in, or throttled?
6. **Metadata format.** Raw API JSON (python-github-backup style), a forge-importable dump (Forgejo `dump-repo`), or a custom schema? Is restoring to a live forge a goal, given that the API cannot preserve numbers, authors, or timestamps?
7. **Forge coverage.** GitHub only at first, or GitLab, Codeberg/Forgejo, and Bitbucket too? Each has different metadata APIs and rate limits (§4).
8. **Storage backends.** Local bare repos, bundles, S3-compatible storage, IPFS, BitTorrent, push-to-forge, SWH submission, or a combination? What is the source of truth, and what is merely distribution?
9. **Distribution and re-publication posture.** Should archives be re-published publicly (e.g. pushed to Codeberg or GitHub)? Re-posting DMCA'd content invites the same process (§5, §6). Or should they be held privately, with publication left to the operator?
10. **Detection.** Should the tool distinguish DMCA (451 via REST) from deletion or suspension (404)? Should it record `block.html_url` and track counter-notices and retractions for restoration? REST is required for this, because GraphQL cannot tell the cases apart.
11. **Authentication and rate budget.** Authenticated only? Personal token vs. GitHub App? How should secondary-limit backoff and `retry-after` be handled, given GitHub's warning that continuing while limited can lead to a ban (§4.1)?
12. **Integration with existing archives.** Should the tool also trigger SWH Save Code Now (or use SWH as a fallback source)? Should it reuse gickup, ghorg, or python-github-backup, or replace them?
13. **Deployment model.** Single-user CLI, a daemon with a schedule, or a community or federated service (shared watch lists, deduplication across operators)?
14. **Integrity and provenance.** Should archives be signed or hash-listed so third parties can verify a copy matches what was on the forge at time T?
