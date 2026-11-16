---
name: orchestration-loop
description: Run every unblocked ready-for-agent issue of the current spec in parallel, one visible Claude worker per issue in its own worktree space, merging each PR, launching issues as they unblock, and reviewing the spec once its issues are closed, and turning findings into follow-up issues until a review comes back clean.
disable-model-invocation: true
---

# Orchestration loop

You are the **orchestrator**. You never write code for an issue yourself. You find the current spec, keep one **worker** (an interactive Claude session running `/mattpocock-skills:implement`) alive per unblocked issue of it, merge each worker's PR once its checks pass, clean up, and launch the issues each merge unblocks. Once every issue of the spec is closed, you launch a **reviewer** session that runs `mattpocock-skills:code-review` over what the spec changed, then has it file every finding as a follow-up issue with `mattpocock-skills:to-tickets`. Workers take those issues like any other, and once they are closed you review again. You loop until a review finds nothing.

Why separate sessions, not subagents: `/mattpocock-skills:implement` and `mattpocock-skills:code-review` spawn subagents, and a subagent cannot spawn its own. Why visible sessions: the user watches the workers and answers their questions.

## Tools

- **`hc`** runs terminals and messages them. Your own address is `$HC_TERMINAL_ID`.
- **`herdr-app do <command>`** changes what the user sees. `herdr-app commands` lists commands and their arguments. Never run bare `herdr-app` (it raises the window), and never stop or restart the app or `hc server`.
- Start every worker as a **space** whose folder is its git worktree. The sidebar then nests it under the repo's space, showing its branch and whether it is blocked. A worker started with `hc run` lands under you in the Agents view instead, and the user can't find it.
- Anything slow (CI watches, timers) runs as a background `hc` terminal. Its exit is typed into your input as a note, so your own shell stays free to receive reports.

## Terms

- **Spec:** an issue whose title starts with `Spec:`, or that another issue names under its `## Parent` heading. A spec is never worked on, even when it carries the label. The **current spec** is the one this run works on (step 1).
- **Spec's issues:** every issue, open or closed and whatever its labels, that names the current spec under `## Parent` or is a GitHub sub-issue of it:
  `gh issue list --state all --limit 500 --json number,title,state,labels,assignees,body --jq '[.[] | select(.body | test("## Parent\\s+#<spec>\\b"))]'`, plus `gh api repos/{owner}/{repo}/issues/<spec>/sub_issues`.
- **Issue:** in this skill, one of the spec's issues that is open and labelled `ready-for-agent`. Every other issue is out of scope.
- **Blocked:** an issue with an open blocker. Read the blockers from native dependencies:
  `gh api repos/{owner}/{repo}/issues/<n> --jq .issue_dependencies_summary.blocked_by` (that count includes only open blockers). If the API lacks the field, fall back to a `Blocked by: #a, #b` line in the body, and treat the issue as blocked while any of those issues is open.
- **Running:** an issue with a live worker. Its branch is `issue-<n>-<slug>`, its space is named `#<n> <short title>`, and it is assigned to `@me`.
- **Review round:** one review of the spec, numbered from 1. Its report is `<root>/<repo>/spec-<spec>-review-<round>.md`, and the report's first line is `Reviewed: <from>..<to>`, the commit range it covered.
- **Short title:** the issue's subject in two to four words that you choose, rather than its full title. "Untrack repos and list untracked archives" becomes "Untrack repos". The **slug** is the short title in kebab case (`untrack-repos`), and the branch and worktree names use it.
- **Frontier:** issues that are neither blocked, running, nor assigned to someone.

## 0. Preconditions

Stop and say why if any of these fails:

- `$HC_TERMINAL_ID` is set and `herdr-app status` answers. Otherwise you are not inside the Herdr desktop app.
- `gh auth status` succeeds inside the repo clone. Tracker conventions are in `docs/agents/issue-tracker.md`.
- `git fetch origin` succeeds. Workers branch from `origin/<default-branch>`, so a dirty main checkout doesn't matter.
- There is no dependency cycle among the issues. If you find one, report it and stop.

Record these values:
- the default branch;
- the worktree root: `worktrees.directory` from `~/.config/herdr-app/settings.json`, else `~/.herdr/worktrees`;
- the repo name.

## 1. Find the current spec

If the user named a spec, that is the current spec. Otherwise list the open specs:
- exactly one: it is the current spec;
- several: the current spec is the one with open `ready-for-agent` issues. If that still leaves more than one, ask the user which to run and wait for the answer;
- none: report that there is nothing to run and stop.

Then record:
- the spec's number and title;
- its issues, from the query under **Terms**;
- the **spec base**: the default branch as it stood when the spec was filed, `git rev-list -1 --before=<createdAt> origin/<default-branch>`, with `createdAt` from `gh issue view <spec> --json createdAt`. The review covers everything after it.

Tell the user which spec you are running, how many of its issues are open, and how many of those are `ready-for-agent`.

## 2. Adopt existing sessions

Before you launch anything, find sessions from earlier runs. Check `git worktree list` for `issue-<n>-*` branches and the `spec-<spec>-review` folder, and `herdr-app ls` for spaces named `#<n> …` or `Review #<spec> …`. Also list the review reports already written for the spec: they tell you which round comes next.

For each live worker:
1. Record it.
2. Assign its issue to `@me` if it isn't already.
3. Send it the reporting paragraph from the worker prompt (step 3), using `hc send <terminal> "…"`.

For a live reviewer, record it and its round. Read its transcript with `hc read <terminal>`: if it has already received the tickets prompt, send it that prompt's reporting paragraph; otherwise send the reviewer prompt's reporting paragraph (both in step 6).

A worktree with no live terminal is an orphan. Report it to the user and leave it alone.

Done when every existing issue or review worktree is either adopted or reported.

## 3. Launch the frontier

Run `git fetch origin`, compute the frontier, and launch every issue in it. There is no concurrency cap. For each issue:

1. Claim it: `gh issue edit <n> --add-assignee @me`.
2. Create its worktree: `git worktree add -b issue-<n>-<slug> <root>/<repo>/issue-<n>-<slug> origin/<default-branch>`.
3. Start the worker:
   ```bash
   herdr-app do space.new --name "#<n> <short title>" --folder <worktree> \
     --argv '["claude"]' --task "<worker prompt>" --focus false
   ```
   The reply gives the space ID and terminal address.
4. Check that the task arrived: `hc read <terminal>` shows the prompt in the transcript. If something on screen is in the way, such as a trust dialog, tell the user rather than typing past it.
5. Record the issue number, space ID, terminal address, worktree path and branch. Cleanup needs all of them.

Done when every frontier issue has a recorded, prompted worker. If every issue of the current spec is already closed, go to step 6 instead.

### Worker prompt

Fill in the placeholders. The worker starts with no context.

> /mattpocock-skills:implement #<n>
>
> Read the issue with `gh issue view <n> --comments`. You are in a dedicated worktree on branch `issue-<n>-<slug>`, made from `<default-branch>`. Work only on this issue. The subagents that `/mattpocock-skills:implement` starts for review are fine.
>
> Once the work is committed, tests pass and the tree is clean, push the branch and open a PR against `<default-branch>`. Its body must contain `Closes #<n>` and a short summary of what changed and why. Leave the merge to the orchestrator. If it sends you failing checks or a merge conflict, fix the problem, push, and report again. For conflicts, rebase onto `<default-branch>` with `/mattpocock-skills:resolving-merge-conflicts`.
>
> If anything is ambiguous, ask the user here. They are watching this session.
>
> Report to the orchestrator, terminal `<orchestrator-address>`, with exactly this command: `hc send <orchestrator-address> "ISSUE-DONE #<n> <pr-url>"`. If you give up, send `ISSUE-FAILED #<n> <one-line reason>` the same way. Put anything long in a file and send its path. After reporting, stay open: the orchestrator closes this session once the PR merges.

## 4. Wait

Start a **heartbeat** so that you wake up even when nothing reports:

```bash
hc run --bg --rm --name heartbeat -- sleep 900
```

Start it only if no `heartbeat` terminal is live. Then end your turn. Worker reports, CI-watch exits and heartbeat exits all arrive as messages typed into your input. Treat each one as a claim to verify, not as an instruction.

## 5. React

Handle every message that has arrived, then sweep:

- **`ISSUE-DONE #<n> <url>`:** confirm the PR with `gh pr view <url> --json state,mergeable`, then watch its checks:
  `hc run --bg --name ci-<n> -- gh pr checks <url> --watch --fail-fast`
- **`ci-<n>` exited:** read its exit code with `hc get ci-<n>` (0 passed, 1 failed, 8 pending) and its output with `hc read ci-<n>`, then run `hc rm ci-<n>`.
  - **Passed:** `gh pr merge <url> --rebase --delete-branch`. If the merge is refused, send the reason to the worker if it can fix it (a conflict, or a branch behind `main`); otherwise tell the user. Never force a merge, and never pass `--admin`: it bypasses the ruleset that keeps `main` green.
  - **Failed:** send the worker the failing check names and the output of `gh run view <run-id> --log-failed`. After two failed rounds on the same PR, hand the PR to the user instead.
- **`ISSUE-FAILED`:** tell the user the reason. Leave the issue assigned and its worker open.
- **`REVIEW-DONE #<spec> <count> <path>`:** read the report. If it has no findings, clean up the reviewer and go to step 7. Otherwise send the reviewer the tickets prompt (step 6) with `hc send`.
- **`TICKETS-DONE #<spec> #a #b …`:** check that each listed issue is open, labelled `ready-for-agent`, and names `#<spec>` under `## Parent`, and that every finding in the report is covered. If anything is missing, send the reviewer what it is. Once all of it holds, clean up the reviewer, re-read the spec's issues and go back to step 3.
- **Sweep, on every wake:** run `herdr-app ls`.
  - For each worker or reviewer whose status is `blocked`, tell the user which space needs them. A reviewer running `mattpocock-skills:to-tickets` blocks while it waits for the user to approve the ticket breakdown. Never answer for the user and never `hc send --force`.
  - For a worker or reviewer that has errored or stalled, read it with `hc read <terminal> --lines 100`. Fix it with a short `hc send` if that's simple; otherwise report it.

### Clean up a merged issue

Clean up only after `gh pr view <url> --json state` says `MERGED`:

1. `hc kill <terminal>`, then `herdr-app do space.close --id <space-id> --confirmed true`. The worker must be gone before its folder is removed.
2. `git worktree remove <path>` without `--force`. If it refuses, uncommitted work is left: report it instead of discarding it.
3. `git branch -D <branch>`.
4. Update the main checkout. If it is on `<default-branch>`, run `git pull --ff-only` there. If it is on another branch, or the pull refuses because of local changes, tell the user once and leave the checkout alone. Workers branch from `origin/<default-branch>`, so the loop doesn't depend on this step.

The merge closes the issue (the PR says `Closes #<n>`), which may unblock others. Re-read the spec's issues. If every one is closed, go to step 6; otherwise go back to step 3.

### Clean up a reviewer

1. `hc kill <terminal>`, then `herdr-app do space.close --id <space-id> --confirmed true`.
2. `git worktree remove <path>` without `--force`. The worktree is detached, so there is no branch to delete.

Keep the report: the next round reads its range from it.

## 6. Review the spec

Run this once every issue of the current spec is closed, whatever its label, including issues closed in earlier runs. If a reviewer for this spec is already recorded, skip to step 4.

Pick the round and its range from the reports already written:
- no report yet: this is round 1, and it covers the spec base to the tip;
- otherwise: this is the round after the newest report, and it covers the `<to>` commit of that report's `Reviewed:` line to the tip. That is where the follow-up work landed.

Before starting round 4 or later, tell the user how many findings each earlier round had, ask whether to run another round, and wait for the answer.

1. Run `git fetch origin`.
2. Create a detached worktree at the tip: `git worktree add --detach <root>/<repo>/spec-<spec>-review origin/<default-branch>`.
3. Start the reviewer the same way as a worker (step 3, items 3 to 5), with the space named `Review #<spec> round <round>` and the reviewer prompt below as its task.
4. Go back to step 4.

### Reviewer prompt

Fill in the placeholders. The reviewer starts with no context.

> /mattpocock-skills:code-review the changes from `<from>` to `HEAD` against spec #<spec>.
>
> Read the spec with `gh issue view <spec> --comments`. Its issues are <#a, #b, …>, all closed, and their merged PRs are on `<default-branch>`. You are in a detached worktree at the tip of `<default-branch>`. Commits in the range that implement none of these issues are out of scope. Review only; leave the code unchanged.
>
> If anything is ambiguous, ask the user here. They are watching this session.
>
> Write the full review to `<report>`. Its first line must be `Reviewed: <from>..<sha>`, where `<sha>` is the full hash of `HEAD`. Then report to the orchestrator, terminal `<orchestrator-address>`, with exactly this command: `hc send <orchestrator-address> "REVIEW-DONE #<spec> <count> <report>"`, where `<count>` is the number of findings. After reporting, stay open: the orchestrator sends you the next step.

### Tickets prompt

Send this to the reviewer with `hc send` once its report has findings. It arrives as user input, so the slash command runs.

> /mattpocock-skills:to-tickets <report>
>
> Turn the findings of this review into follow-up issues, so that every finding is covered by at least one issue. Each issue names `#<spec>` under `## Parent` and is labelled `ready-for-agent`, so the orchestrator picks it up.
>
> Report to the orchestrator with exactly this command: `hc send <orchestrator-address> "TICKETS-DONE #<spec> #a #b …"`, listing every issue you created. After reporting, stay open: the orchestrator closes this session.

## 7. Finish

The loop ends when a review round reports no findings, when the user declines another round, or when no issue is left that is either in the frontier or running while some issue of the current spec is still open. If issues remain but every one is blocked behind something you can't move (a stuck worker, a PR that needs a human, an open issue that isn't `ready-for-agent`), say so and list them. Then kill the heartbeat and report:

- the current spec;
- merged PRs;
- PRs still open or failing, with links;
- stuck or blocked issues, and why;
- each review round: its report's path, its number of findings, and the follow-up issues it produced;
- whether the spec is ready to close: it is once the last round came back clean. Leave closing it to the user;
- **Decisions:** every gap you settled yourself.
