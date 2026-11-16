# git-archiver

## Agent skills

### Issue tracker

Issues and specs live in this repo's GitHub Issues, managed with the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default canonical labels: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `CONTEXT.md` plus `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## Pull requests

`main` keeps a linear history.

- Before opening a PR, run `git fetch origin`, rebase your branch onto `origin/main`, and rerun the tests on the result. If the branch is already pushed, push with `--force-with-lease`.
- Merge with `gh pr merge --rebase`. The repo allows no other merge method.
