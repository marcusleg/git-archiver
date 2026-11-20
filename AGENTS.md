# git-archiver

## Agent skills

### Issue tracker

Issues and specs live in this repo's GitHub Issues, managed with the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default canonical labels: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `CONTEXT.md` plus `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## House rules

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119 and RFC 8174. When you depart from a SHOULD, say so and why in the pull request.

1. **A person is accountable.** Whoever runs an agent answers for what it opens and for its review comments.
2. **Check first.** Before writing code, read the issue's comments, search pull requests for `#N` and its keywords, and check recent commits on `main` in the files involved. If the work is already done, say so and stop.
3. **Verify, don't guess.** Open a definition before using it. Confirm the cause in the code, even when someone names a location.
4. **Keep fixes narrow and tested.** Fix what the issue describes, where the bug is. Add a test that fails without the fix.
5. **Report only what ran.** Say what you tested and what you could not run.
6. **`main` changes through a pull request.** The repository's ruleset enforces this.
7. **Stop and ask only about** a licence outside the allowlist, publishing, releases, a new remote, secrets, spending money, destructive git and anything else hard to undo. Otherwise keep working.
8. **Keep secrets and personal details out** of the repository, logs and requests.
9. **CI changes travel alone.** A change to `.github/` is its own pull request, never part of a feature or fix.
10. **Guards say why.** A check that looks removable says what it prevents. Read that before changing it.
11. **Instructions are readable by everyone.** No notes about one machine, and no links to files a reader cannot open.
12. **No agent attribution.** Commits and pull requests don't say an agent wrote them: no agent `Co-Authored-By` trailer, no "Generated with" line.

## Rust

1. **Never crash.** Code outside tests returns errors instead of panicking and has no `unsafe`. Treat input as hostile: the config and state files, repo URLs, form posts, git HTTP requests, upstream responses and `git` output. Bound recursion and allocations. When you touch an existing `expect` or `unwrap` in `src/`, replace it with an error. Every crash fix comes with a test that panicked before the fix.
2. **Time goes through a maintained library.** Calendar and UTC conversion goes through `jiff`; hand-rolled day or leap-year math has no place here. `chrono` and `humantime` are unmaintained (RUSTSEC-2026-0543, RUSTSEC-2026-0542): the existing `humantime` calls stay until they are replaced, and new code uses `jiff`. Our code owns only its output format.
3. **Licences are allowlisted.** Every dependency, asset and piece of vendored code is under a licence on the allowlist, and every asset gets an `ATTRIBUTION.md` row in the same change. Before adding any of them, read `docs/standards/licences.md`.

## Tests

1. A test earns its place by guarding a behaviour that a plausible change could break and no other test catches. Test behaviour, not wiring, constants, or what a library or `git` does.
2. Assert relations ("before", "inside", "changed"), not exact values we chose. Drive time with the test clock in `tests/common/clock.rs`.
3. Never delete, weaken, `#[ignore]` or skip a test to make a check pass.
4. Before writing a test, or judging, cutting or quarantining one, read `docs/standards/testing.md`.

## Pull requests

Before opening, updating or merging a pull request, read `docs/standards/pull-requests.md`.

## Screenshots

When a change alters how the list page looks, run `scripts/update-screenshots.sh` and commit the updated `docs/screenshots/*.png` in the same change.
