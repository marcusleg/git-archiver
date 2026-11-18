---
name: cut-a-release
description: Cut a release of git-archiver: pick the semantic version from what changed since the last release, bump it through a PR, tag main, then put the notes on the GitHub Release the workflow publishes. Use when the user asks to release, cut or ship a version, or bump the version.
---

A release is a `vX.Y.Z` tag on main. The tag push runs `Release` (`.github/workflows/release.yml`), which refuses a tag that doesn't match `Cargo.toml`'s `version`, then builds and smoke-tests the amd64 and arm64 binaries and image, publishes `ghcr.io/marcusleg/git-archiver:X.Y.Z`, `:X.Y` and `:latest`, and creates the GitHub Release with the binaries attached. You then replace that Release's generated notes with yours.

## Versioning

The **contract** is what an operator relies on across upgrades:

- `--data-dir` / `GIT_ARCHIVER_DATA_DIR` and `RUST_LOG`;
- the `config.toml` settings, and which files the app accepts;
- the data directory: `state.json`, the archive paths under `repos/` (derived from the repo identity), and the preserved ref names `refs/preserved/<time>/{heads,tags}/<name>`;
- the image's port 8080, volume `/data` and uid 10001 (a host-mounted data directory is owned by that uid);
- clone URLs under `/git/`, metric names and labels under `/metrics` (alerts query them), and the release asset names (install scripts download them);
- the features themselves.

The web UI's HTML and form routes serve the app's own pages and are not part of it.

- **Major**: upgrading needs action from the operator or breaks something they rely on: an option or environment variable renamed, removed or newly required; a setting renamed or removed, or a config that loaded before now rejected; a changed port, volume or uid; a data directory the new version can't use without a manual step, or that loses data; changed archive paths, ref names, clone URLs, metrics or asset names; a feature removed.
- **Minor**: new features, a new optional setting or environment variable, a new metric, and **any change to what the app writes to the data directory** that the new version still reads from the old one. Rolling back across a patch is then always safe; across a minor it may not be.
- **Patch**: everything else that reaches the binary or the image: fixes, dependency and base-image updates, performance, wording.

While the version is `0.x`, a major change bumps the minor (`0.4.2` → `0.5.0`) and gets upgrade notes; minor and patch apply as above. Moving to `1.0.0` is the user's call; propose it, never pick it.

A release with no change that reaches the binary or the image (only docs, tests, CI, agent skills) has nothing to ship: tell the user and stop.

A **pre-release**, only when the user asks for one, is `X.Y.Z-rc.N`: `X.Y.Z` picked as below, `N` one past the last `vX.Y.Z-rc.*` tag. The workflow marks its Release a pre-release and leaves `:X.Y` and `:latest` alone.

## 1. Collect the changes

```sh
git fetch origin --tags
last=$(git describe --tags --abbrev=0 --match 'v*' --exclude '*-*' origin/main)   # fails: first release, use the whole history
git log --format='%h %s' "$last"..origin/main
git diff --stat "$last"..origin/main -- src/main.rs src/config.rs src/default_config.toml src/state.rs src/archive.rs src/identity.rs src/git_http.rs src/metrics.rs Containerfile .github/workflows/release.yml
```

Main is linear and a PR lands as several commits, so subjects carry no PR number. Find it with `gh api repos/{owner}/{repo}/commits/<sha>/pulls --jq '.[].number'`; a commit pushed straight to main has none.

Classify every commit as _major_, _minor_, _patch_ or _not shipped_ against **Versioning**. A subject is a hint, not proof: read the diff (`git show <sha>`) of every commit touching a file in the `git diff --stat` list above, since that is where a contract change hides.

Done when every commit in the range carries a class.

## 2. Pick the version and write the notes

The version is `$last` bumped by the highest class in the range; the first release is `0.1.0`.

Write the release notes for operators, in their words, not the commits'. One line per change, PR number at the end (`(#56)`); fold all dependency and base-image updates into one line. Sections, empty ones left out:

```markdown
## Upgrade notes
What an operator must do before or after upgrading to this version, step by step.

## Features
## Fixes
## Other changes

Container image: `ghcr.io/marcusleg/git-archiver:X.Y.Z` (linux/amd64, linux/arm64). Static binaries for Linux amd64 and arm64 are attached below.
```

Every major-class change needs an entry under _Upgrade notes_. Save the notes to the scratchpad.

Show the user the version, the reason for it (the change that set the class) and the notes. Go on only with their go-ahead; they may change the version or the notes.

## 3. Bump the version through a PR

When `Cargo.toml` on `origin/main` already carries the version (the first release), skip to step 4 with the tip of `origin/main` as the release commit.

```sh
git switch -c release-X.Y.Z origin/main
sed -i '0,/^version = ".*"$/s//version = "X.Y.Z"/' Cargo.toml
cargo update --workspace --offline   # moves only git-archiver's own entry in Cargo.lock
cargo test --locked
git commit -am "Release X.Y.Z"
git push -u origin release-X.Y.Z
gh pr create --title "Release X.Y.Z" --body-file <notes>
gh pr checks --watch
gh pr merge --rebase --delete-branch
```

Done when the PR is merged. A rebase merge rewrites the commit, so take the release commit from GitHub, not your branch: `gh pr view --json mergeCommit --jq .mergeCommit.oid`.

If commits landed on main between step 1 and the merge, they are in the release: classify them, and if they raise the class, bump again in a new PR; otherwise add them to the notes.

## 4. Tag and publish

```sh
git fetch origin
git tag -a vX.Y.Z <release commit> -m "Release X.Y.Z"
git push origin vX.Y.Z
gh run list --workflow release.yml --branch vX.Y.Z   # then: gh run watch <id> --exit-status
```

Done when the run is green. Then replace the generated notes with yours:

```sh
gh release edit vX.Y.Z --notes-file <notes>
```

Report the release URL to the user.

A run that failed before the `publish` job's `Publish the multi-arch image` step published no image and no Release: delete the tag (`git push origin :refs/tags/vX.Y.Z`, `git tag -d vX.Y.Z`), fix the cause and tag again. Once that step has succeeded, the tag is permanent: retry a later failure with `gh run rerun <id> --failed`, and fix anything else forward with a patch release.
