---
name: orchestrate-tickets
description: Orchestrate implementing a set of GitHub tickets in dependency order and in parallel, by launching one visible, interactive Claude session per ticket in its own git worktree inside herdr, each of which opens a PR. Use whenever the user wants to "work through the tickets", "implement the spec in parallel", "orchestrate the issues", "kick off the ready tickets", or run /implement across several issues at once, even if they don't mention herdr or worktrees. Requires running inside a herdr pane (HERDR_ENV=1).
---

# Orchestrate tickets

You are the **orchestrator**. You do not implement anything yourself. You read the ticket graph, launch one interactive Claude session per ready ticket, route messages between workers and the user, and merge each PR once its checks pass, clean up, and launch the tickets that become unblocked.

Why separate top-level sessions instead of subagents: `/implement` drives `/tdd` and `/code-review`, and `/code-review` spawns its own subagents. A subagent cannot spawn subagents, so each ticket needs its own full session. Why visible herdr panes: the user wants to watch, and workers must be able to stop and ask them questions.

## 0. Preconditions

Check these first and stop with a clear message if one fails:

- `test "${HERDR_ENV:-}" = 1`. If it fails, say you are not inside herdr and stop. Do not control a herdr session from outside it.
- `gh auth status` works, and you are in the repo clone (`docs/agents/issue-tracker.md` has the tracker conventions).
- The main checkout is clean (`git status --porcelain` is empty) and up to date: `git fetch origin`.
- Note your own pane ID: `echo "$HERDR_PANE_ID"`. Workers use it to report back to you.

Learn the installed syntax from the binary rather than trusting this file: `herdr worktree create --help`, `herdr agent` (and `herdr agent wait --help`, `herdr agent prompt --help`). Responses are JSON; read IDs from them instead of predicting them. Do not run bare `herdr` (it launches the TUI).

## 1. Decide the scope

The argument is one of: a spec/map issue number (its children are the tickets), a list of issue numbers, or nothing. With nothing, use every open issue labelled `ready-for-agent`. State which interpretation you used.

Fetch each ticket with `gh issue view <n> --json number,title,body,labels,assignees,state` and its blockers:

```bash
gh api repos/{owner}/{repo}/issues/<n> --jq '{id, blocked_by: .issue_dependencies_summary.blocked_by}'
```

If native dependencies are unavailable, fall back to a `Blocked by: #a, #b` line in the ticket body. Build the graph yourself and check for cycles. Report a cycle and stop.

Settle small gaps yourself rather than interrupting the user, and list each one under "Decisions" in your final report. Defaults:

- **Concurrency cap:** 8 sessions at once. Parallel tickets can still touch the same files, so expect the occasional PR conflict; workers resolve those (see the worker prompt).
- **PR target:** the repo's default branch. A dependent ticket starts only once its blockers' PRs are **merged** (which now happens automatically), so it branches from a base that contains their code and its PR diff stays clean.
- **Who merges:** you, automatically, once the PR's checks pass (see "Check and merge"). Use `--squash` unless the repo's history or docs show another convention. If the PR has no checks configured at all, there is nothing to wait for, so merge it and note that in the Decisions list.
- **Branch names:** `ticket-<n>-<short-slug>`.

## 2. The loop

Repeat until every ticket is merged, or only blocked/stuck ones remain:

1. **Frontier:** open tickets with no open blockers (blockers close when their PR merges, since PRs say `Closes #<n>`), no assignee, and not already running. Take up to the cap minus the sessions in flight. Run `git fetch origin` first so new worktrees see freshly merged work.
2. **Launch each ticket:**
   1. Claim it: `gh issue edit <n> --add-assignee @me`. This stops a second orchestrator or a human from picking it up.
   2. `herdr worktree create --branch ticket-<n>-<slug> --base origin/<default-branch> --no-focus` (add `--trust-repository` only if herdr asks and the repo is the user's own). The response identifies the new workspace and its root pane. Keep a table of ticket → workspace ID, pane ID, branch and agent name; you need all four at cleanup.
   3. `herdr agent start t<n> --kind claude --pane <pane-id>`. Agent names are `[a-z][a-z0-9_-]{0,31}` and unique among live agents. The pane must be at a shell prompt, which a fresh worktree pane is. Leave Claude in its default interactive mode so the user can talk to it. If the user told you to pass flags such as a permission mode, pass them after `--`.
   4. `herdr agent prompt t<n> "<worker prompt>"` without `--wait`, so you can launch the others in parallel.
3. **Wait for news.** Reports arrive as prompts typed into your own session (see below), so you usually just end your turn and handle them as they land. Between reports, `herdr agent list` shows every worker's state, and `herdr agent wait` (read its `--help`) can wait on one.
4. **React to what arrives:**
   - **A worker reports `TICKET-DONE`:** record the PR URL and go to "Check and merge". Leave the worker's pane open until the PR merges, since a failing check needs the worker's context.
   - **A worker is `blocked` or visibly asking a question:** do not answer for the user. Tell the user which agent (`t<n>`, with its workspace) needs them. They can focus that pane and reply. Keep managing the other tickets meanwhile.
   - **A worker is stalled or errored:** `herdr agent read t<n>` to see why. Re-prompt for a simple fix; otherwise report it and leave the ticket assigned.
5. If every remaining ticket is blocked on something you can't resolve (a stuck worker, a PR that needs a human), say so plainly and list them.

### Check and merge

For each reported PR, wait for its checks without polling: run `gh pr checks <url> --watch --fail-fast` as a background command, so you are re-invoked when it exits. Then:

- **Checks passed:** `gh pr merge <url> --squash --delete-branch`, then clean up (below). If the merge is refused (conflict, required review, branch protection), do not force it. Send the reason to the worker with `herdr agent prompt t<n> "..."` if it is something the worker can fix (a conflict), otherwise tell the user.
- **Checks failed:** send the failing check names and `gh run view --log-failed` output to the worker with `herdr agent prompt t<n>`. The worker fixes, pushes and replies `TICKET-DONE` again, and you re-watch. After two failed rounds on the same PR, stop and tell the user instead of looping.
- **"No checks reported":** see the Who-merges default above.

### Clean up after a merge

Once `gh pr view <url> --json state` says `MERGED` (don't trust the merge command's exit alone):

1. Stop the agent by closing its pane: `herdr pane close <pane-id>`. Closing it ends the Claude session, so it can't write into a worktree you are about to delete.
2. `herdr worktree remove --workspace <workspace-id>`. Without `--force`: a refusal means uncommitted work is left, so report it rather than discarding it.
3. Delete the local branch (`git branch -D ticket-<n>-<slug>`; `--delete-branch` already removed the remote), `git fetch origin`, and `git pull --ff-only` on the main checkout.
4. Recompute the frontier. Merging a blocker is what unblocks its dependents.

### Worker prompt

Give each worker everything it needs, since it starts with no context. Fill in the placeholders, including your pane ID:

> Run `/implement` on GitHub issue #<n> (`gh issue view <n> --comments`). You are in a dedicated git worktree on branch `ticket-<n>-<slug>`, branched from `<default-branch>`. Work only on this ticket.
>
> When the work is committed, tests pass and the tree is clean: push the branch and open a PR against `<default-branch>` with `gh pr create`. The body must contain `Closes #<n>` and a short summary of what changed and why. Don't merge it; the orchestrator merges once checks pass. If checks fail, the orchestrator will message you the details, so fix them and report again. If the PR conflicts with newer work on `<default-branch>`, rebase it and resolve the conflict (`/resolving-merge-conflicts`).
>
> If anything is ambiguous or blocked, ask the user in this session. They are watching.
>
> Then report to the orchestrator, which is the agent in pane `<orchestrator-pane-id>`. Run exactly:
> `herdr agent prompt <orchestrator-pane-id> "TICKET-DONE #<n> <pr-url>"`
> Don't pass `--wait`. Send the same command with `TICKET-FAILED #<n> <one-line reason>` if you give up. After reporting, stay open: the orchestrator closes your pane after the merge.

Why `herdr agent prompt` and not `hc send`: the workers are herdr-launched agents, not `hc` children, so there is no `hc` parent link to carry notes. `herdr agent prompt` types into the orchestrator as an ordinary message, and Claude Code queues such messages while you are mid-turn, so reports are not lost. It is also rejected if you are sitting at an approval dialog, in which case the worker sees the error and should retry shortly.

## 3. Final report

When the loop ends, tell the user in a few lines: which tickets have merged PRs, which PRs are still open or failing (with links), which are stuck and why, and a **Decisions** list of every gap you settled yourself (scope interpretation, concurrency cap, merge method, merging PRs with no checks, branch names, anything else).

## Boundaries

- Never implement ticket code yourself, and never edit a worker's worktree. Concurrent edits corrupt its state.
- Merge only PRs whose checks passed, never with `--admin` or by bypassing branch protection. Never force-push, and never close an issue by hand (the PR's `Closes #<n>` does it).
- Never answer a blocked worker's question for the user.
