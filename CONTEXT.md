# Git Archiver

A self-hosted tool that keeps local copies of public git repositories so their history survives takedowns, deletions and rewrites upstream.

## Language

### Repos and archives

**Tracked repo**:
A git repository listed in the config, identified by its URL, that the app keeps fetching.
_Avoid_: watched repo, mirror, source

**Archive**:
The local copy of one repo's default branch and tags, including every preserved ref. An archive outlives tracking: it is only deleted when a person explicitly asks for that in the UI.
_Avoid_: backup, mirror, clone

**Untrack**:
To remove a tracked repo from the config. Untracking never deletes the archive by itself; removal through the UI asks separately whether to delete it.
_Avoid_: remove, delete (when only untracking is meant)

**Untracked archive**:
An archive still on disk whose repo is no longer tracked. Tracking the same repo again reattaches it.
_Avoid_: orphan, stale archive

**Repo identity**:
A repo's normalized URL. Scheme, host case, trailing `.git` and slashes are ignored, so `http://` and `https://` name the same repo. Two URLs with the same identity are the same repo and share one archive.

### Change detection

**Rewrite**:
An upstream change that would lose history held in the archive: the default branch's new tip doesn't descend from the archived tip, a tag moved or was deleted, or the default branch was switched to another branch.
_Avoid_: force-push (that's one cause of a rewrite, not the concept)

**Preserved ref**:
A ref in the archive that holds a tip which existed before a rewrite, kept forever.
_Avoid_: snapshot, previous copy, backup

### Scheduling and status

**Fetch**:
Updating one archive from its upstream: checking for rewrites, preserving what a rewrite would lose, then taking the new state. A newly tracked repo's first fetch is its clone.
_Avoid_: backup, sync, archive action

**Interval**:
The global period within which every tracked repo is fetched once. Fetches are spread evenly across it, one repo at a time, the repo with the oldest last attempt first, whether that attempt succeeded or not.
_Avoid_: cycle, backup interval

**Available**:
The last fetch of a tracked repo succeeded.

**Unavailable**:
Upstream refused the repo with a 4xx response (e.g. DMCA takedown, deletion). The repo keeps its turn in the interval, and becomes available again if upstream restores it.
_Avoid_: dead, taken down (one cause of unavailability, not the state)

**Fetch failed**:
A transient failure (5xx, network error, timeout, or a 408 or 429 that only asks to come back later) that says nothing about the repo's fate. It is retried at the repo's next turn and never touches the archive.

### Config

**Config error**:
Why the config file was rejected. It is shown at startup, in the log, and on the list page while the app keeps running on the last good config, so anyone may read it. It never quotes what the operator wrote.
_Avoid_: invalid config message, parse error, config warning (that's the banner on the list page that shows it)
