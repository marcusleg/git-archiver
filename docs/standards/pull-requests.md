# Pull requests

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119 and RFC 8174.

These rules apply to every pull request in this repository, by a person or an agent. `AGENTS.md` carries the short form and links here. Adapted from [storytold/craft-contrib's pull request standard](https://github.com/storytold/craft-contrib/blob/main/standards/pull-requests.md); this copy is the one that applies here.

`main` keeps a linear history. The `main: CI green and up to date` ruleset requires a pull request, the `check` and `image` jobs green on a branch that is up to date with `main`, and rebase merges only.

## Shape

1. A pull request SHOULD fix only what the issue describes, where the bug is, and SHOULD fix the same bug on other paths in the same pull request. For a different bug, open an issue unless one exists. It SHOULD NOT change code the fix does not need: no refactors, no formatting.
2. Moving code and changing behaviour SHOULD be separate pull requests.
   - A move SHOULD keep behaviour. Its description SHOULD say which tests pass before and after, with counts.
   - A behaviour change SHOULD add a test that fails without it, unless [testing.md](testing.md)'s C1 exemption applies, and SHOULD list the tests whose result changes.
3. A CI change (`.github/`) SHOULD be its own pull request.
4. Title and commits SHOULD follow the style of the last commits on `main`: a sentence in the imperative saying what changes for the user, no prefix.
5. A pull request that waits for another SHOULD link it in its description.

## Description

The description SHOULD be short, in this order:

1. First line: what the user gains, for example "A moved tag no longer loses its old commit".
2. `Fixes #N.` only when the pull request fixes the whole issue; otherwise `Part of #N.` and what is left.
3. What the user saw.
4. The cause.
5. What changed, and why the change is this wide and no wider.
6. The test: its name and what it checks.
7. Verification: what ran and where (CI, `cargo test`, a check in the running app), and what did not run. A check that did not run MUST NOT be reported as passed.

Sections that don't apply, such as the cause for a new feature, are left out. Reviewers check the description against this list; no CI check enforces it.

When a change alters how the list page looks, the pull request commits updated `docs/screenshots/*.png`, as `AGENTS.md` says. Other screenshots go in the description, not the repository.

## Before opening

1. Search open pull requests for the issue number and for the functions and files you changed. If another pull request fixes the same thing and is better or equal, don't open yours; say so on the issue.
2. Run `git fetch origin`, rebase your branch onto `origin/main`, and rerun the tests on the result.
3. Review the diff against the issue:
   - every identifier exists as used;
   - the test (if one is required) fails with the fix reverted (run it once that way);
   - nothing outside the issue changed;
   - the description is true.

## Drafts

You SHOULD open a pull request as a draft only while the work is unfinished, and SHOULD mark it ready as soon as it is done and the checks pass. If two different fixes for the same failing check have not worked, leave it a draft, say what fails and what you tried, and stop.

## After opening

1. **Review responses.** You SHOULD respond to an actionable review request within 7 days. A reminder comes at 14 days, and a maintainer MAY close the pull request at 30.
   - The clock starts when an actionable request waits on the author. Optional suggestions, unconfirmed bot findings and a merge conflict do not start it.
   - A fix, an explanation or an agreed plan resets it. Unrelated activity does not.
   - Waiting on a reviewer or a maintainer pauses it.
   - A review comment is a claim to check: fix it if it is a real defect in your change, otherwise reply why not.
2. **Keeping current.** A conflict alone needs no push while the pull request waits for review. When a reviewer asks, or the branch is about to merge, rebase onto `origin/main` as under "Before opening", check that the change is still needed, and push with `--force-with-lease`.
3. **Red `main`.** If a check fails on a test your pull request does not touch, run the same test on `main`. If it fails there too, link the failing `main` run on the pull request and change nothing unrelated. If it passes on `main`, your change caused the failure, and you SHOULD fix it.
4. **Merging.** Merge with `gh pr merge --rebase`; the repository allows no other method.
