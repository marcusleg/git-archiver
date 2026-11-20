# Testing

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in RFC 2119 and RFC 8174.

These rules apply to every test in this repository, written by a person or an agent. `AGENTS.md` carries the short form and links here. Adapted from [storytold/craft-contrib's testing standard](https://github.com/storytold/craft-contrib/blob/main/standards/testing.md); this copy is the one that applies here, and changes to it go through an ordinary pull request.

## Which sections to read

- **Writing a new test:** What tests are for, The keep rule, Cut categories (a new test SHOULD NOT fall in one), Fix categories, Where new tests go, Hostile input, Fixtures, Tests that need a tool, Flaky and hanging tests.
- **Judging or cutting existing tests:** How to judge a test, Cut categories, Fix categories, Examples of kept tests, Cutting existing tests. Existing tests SHOULD be judged and cut only when an issue asks for it.

Three rules hold in both cases:

- You MUST NOT delete, weaken, ignore or skip a test to make a check pass. A quarantine under "Flaky and hanging tests" is the only exception.
- Tests SHOULD be cut or quarantined only in a pull request of their own, which MUST list every removed test.
- You MUST NOT quarantine a test that fails every time.

## What tests are for

Tests specify the invariants of the code: what must stay true however the code changes. Each test SHOULD state one invariant, and fail when a plausible change breaks it.

Every branch of the code SHOULD be reached by an invariant test or be simple enough to check by reading. For a branch that no test reaches, decide which of three it is:

- **A missing invariant:** often an error or edge path in the config parser, the repo identity, or the handling of an upstream response. Write the test.
- **Code simple enough to check by reading:** leave it.
- **Dead code:** delete it.

Coverage points at gaps. It is not a target to reach with tests written for their own sake.

## The keep rule

A test SHOULD stay only if you can write all three of these:

1. **Invariant:** what it guards, in one line.
2. **Breaking change:** a plausible change to the code that breaks that invariant. Plausible means a change someone could make while fixing, refactoring or extending this code: reordering steps, an off-by-one, a dropped case, a changed default, a replaced algorithm. Deleting the function or arbitrary sabotage doesn't count.
3. **Only this test:** no other test would fail on that change. Find the other tests (grep for the function, the route, the status) and check.

If you can't write all three, or are unsure whether you can, the test SHOULD go: the burden is on keeping a test, not on cutting it. The exception is a regression test for losing archived history or for safety: while you are unsure, it SHOULD stay until someone has checked the three lines. Apply this before you add a test. Existing tests are judged only as described under "Cutting existing tests".

The keep rule is necessary, not sufficient: a check in a cut category SHOULD go even when you can write all three lines. For example, readable code (C1) has plausible breaking changes too, but a reviewer catches them by reading.

The cut categories are the common reasons a test fails the keep rule. Name the one that applies. If none fits but the keep rule still fails, write `CUT K` and say which of the three lines you couldn't write.

## How to judge a test

Judge from the test's body and the code it drives, never from its name or file alone.

1. **Split it into checks.** A check is an assertion, or a group of assertions about one thing. Most tests have one check; some have several.
2. **Judge each check:** does a cut category apply, and does it pass the keep rule?
3. **Give a verdict for the test:**
   - Every check is cut: `CUT` with the categories.
   - Some checks are cut, at least one stays: `TRIM`. Delete the cut checks; the test keeps the rest. Rename it if its name no longer fits.
   - No check is cut, but a fix category applies: `FIX`. Rewrite it so that none applies, then judge it again.
   - Otherwise: `KEEP`, with the invariant and the breaking change.

A test, or an assertion inside one, that no CI job runs checks nothing in practice. That covers a test marked `#[ignore]`. If it passes the keep rule, fix it so that CI runs it; otherwise it is C8. A flaky test quarantined under an open issue (see "Flaky and hanging tests") is judged when that issue closes.

**Cite only a test that stays.** A covering test (for C6, or for the "only this test" line) SHOULD itself be kept or trimmed, with the covering check still in it. Two tests that cover each other can't both go: keep the one with more reach, and keep the shared check in it.

Write the verdict as one line per test (the paths, lines and names below are made up and show the format only):

```
tests/it/example.rs:12  a_config_with_a_tab_is_read  CUT C5
tests/it/example.rs:40  the_add_form_posts_to_the_add_route  CUT C2
tests/it/example.rs:71  the_repo_page_lists_tags_and_preserved_refs  TRIM C3 -> keep the preserved-ref check, drop the heading text check
tests/it/example.rs:96  an_untracked_archive_keeps_its_preserved_refs  KEEP guards: untracking never drops history; breaks if: untracking deletes refs under refs/preserved/
tests/it/example.rs:130  fetches_are_eight_hours_apart  FIX F1 -> assert the gaps are equal, not the hours we chose
```

## Cut categories

A check in a cut category SHOULD NOT be written, and SHOULD be cut from an existing test when a cutting pull request is asked for.

### C1 Readable

**Question:** Can a reviewer confirm the code under test is right by reading it, because it is a few lines with at most one branch and no state carried between calls?

- "The code under test" is the code the check exercises, not the whole function around it: a limit check inside a long function is judged by the comparison and its effect.
- **Also C1:**
  - a bug fix whose fixed code is a few lines with at most one branch and no state between calls;
  - a limit enforced by one comparison or one `take(n)`.
- **Not this:**
  - code with a loop that carries state, several interacting branches, or arithmetic whose result isn't obvious, such as spreading fetches across the interval;
  - a limit that interacts with other state.

### C2 Wiring

**Question:** Does the check only show that an input reaches a function: a route calls its handler, a form field is passed through, a CLI flag or environment variable lands in the config?

- **Also C2:** a shared mechanism called from several entry points. Test the mechanism once, where it is defined. Each entry point's call to it is wiring.
- **Not this:** routing logic that decides *which* receiver gets the input. That logic has rules, and a rule can break.

### C3 Default, constant or table entry

**Question:** Does the check assert a default value, a constant, a getter or setter, or an entry of a table or `match`, with no logic between the value and the assert?

- **Example:** a test that the default interval is 24 hours, where the default config just sets it.
- **Not this:**
  - a default computed from other inputs;
  - a property of the whole table that other code relies on: an entry for every variant, no duplicates.
- A table entry stays C3 even when a bug fix added it, and even when its value comes from outside our code (a forge's URL layout, an HTTP status's meaning).
- A default limit is C3 unless the code computes it. A comment explaining where the number comes from is not computation.

### C4 Compiler-checked

**Question:** Would the code fail to compile if the asserted property were false? Examples: a function returns the right type, an enum has a variant, a type is `Send`.

- **Not this:** a runtime property the types don't express.

### C5 Library behaviour

**Question:** Is the behaviour that decides pass or fail implemented in a dependency (axum, maud, serde, tokio, std, any crate outside this repository) or in the `git` program, with our code only calling it?

- If our code around the call is itself C1-readable (a lookup, an argument added, a feature switch), the check is C5.
- **Example:** a test that `serde_json` round-trips a plain derived struct.
- **Example:** a test that `git fetch` brings in a new commit, where our code only runs the command.
- **Not this:** our own logic around the library or `git` that isn't C1: a custom `Deserialize`, a migration, error mapping with branches, deciding from ref lists whether a fetch is a rewrite.

### C6 Duplicate

**Question:** Does another test already go through the same code and fail on the same breaking change? Find that test and confirm it before citing it.

- Of two overlapping tests, keep the one that reaches more of the code. At equal reach, keep the one at the level the user meets the behaviour (through the page, the HTTP endpoint or the CLI), and cut the other.
- **Not this:** a test that reaches a branch the other test doesn't. If only part of it overlaps, TRIM the overlapping checks.

### C7 Combination

**Question:** Is the check one of several that differ only in input values and go through the same branches?

- Keep one per distinct branch; cut the others.
- **Example:** five tests of the repo identity, one per forge host, all through the same normalisation.
- **Not this:** values that land in different branches or edge cases: zero, empty, a port, a trailing `.git`, a 4xx that means "come back later" (408, 429) next to one that doesn't.
- Where both C6 and C7 fit, cite either.

### C8 No real check

**Question:** Does the check run code without asserting anything a regression would change? For example, it renders a page and only checks that it didn't fail.

- **Not this:** "never panics on hostile input" for a parser. There, not panicking *is* the invariant.

## Fix categories

A test in a fix category SHOULD be rewritten so that none applies.

### F1 Exact value

**Question:** Does the check assert exact HTML, CSS, strings, numbers or bytes, where the exact value is our own design choice?

- **Fix:** assert the relation that matters, such as "inside", "before", "changed", "same as before".
- **Not this:** a value fixed outside our code, which is a specification:
  - what the git smart HTTP protocol requires;
  - RFC 3339 timestamps and other formats a standard defines;
  - metric names and labels on `/metrics`, which Prometheus queries and dashboards read;
  - the config and state file formats, which an operator's files and older releases depend on;
  - text an operator relies on, such as a config error's location.

### F2 Timing

**Question:** Does the result depend on wall-clock time, `sleep`, or how fast the machine is?

- **Fix:** drive time with the test clock (`tests/common/clock.rs`), and assert what happened, not how long it took.
  - When a time bound stands in for "A finished before B", assert the order, not the seconds.
  - A deadline that only stops a hanging test (`tokio::time::timeout`) is fine; it ends the test and is not asserted.
  - A test whose point is speed SHOULD NOT be in the CI gate: on a loaded machine it fails without any regression.

### F3 Machine-fragile

**Question:** Could the test pass on one machine and fail on another, without the code under test behaving differently?

- **Typical causes:** absolute path literals; output that depends on the `git` version, the time zone, the locale or the browser window size; floats compared exactly.
- **Fix:** build paths with `Path::join` under the test's temporary directory; set what the test depends on (time zone, window size) explicitly; compare by relation or with a tolerance.

### F4 Unbudgeted stress

**Question:** Does the test loop over many generated or random cases, or build huge inputs?

- **Fix:** test a limit with a small limit value instead of a huge input. Give randomised tests a fixed seed.

### F5 Slow

**Question:** Does a unit test take over 0.1 s, or an integration test over 1 s, without a comment saying why?

- Browser tests pay for a page load; judge them against each other, not against 1 s.
- **Fix:** make it smaller or faster. If the time goes into shared setup, build it once and share it between tests.

### F6 Not a code test

**Question:** Does the test check the repository rather than run code: manifest text, a file listing, a licence field?

- **Fix:** move it into a CI step.

## Where new tests go

- Integration tests SHOULD be one binary: `tests/it/main.rs`, with the test files as its modules (`tests/it/<area>.rs`). Every `.rs` file directly in `tests/` is a separate binary that links the crate and its whole dependency tree again. The existing `tests/*.rs` files predate this rule; until they move, in a pull request of its own, add a test to the existing file for its area and add no new binary.
- Test a shared mechanism once, where it is defined, not in every caller.
- Test behaviour through what the user meets (the page, the HTTP endpoint, the CLI, the archive on disk), not through the helper it calls.
- One invariant per test, named as a sentence about it: `a_force_push_preserves_the_old_tip_and_moves_the_branch`, not `test_rewrite_3`.
- A bug fix SHOULD come with a test that fails without the fix, unless the fixed code is a few lines with at most one branch and no state between calls.
- A crash fix SHOULD come with a test that panicked before the fix, also when the fixed code is that small.
- A change to the config or state file format SHOULD get a test that each value the old version wrote survives the switch.
- A failing generated case SHOULD become a named regression test.

## Hostile input

Code that reads input from outside SHOULD return an error for any input, and SHOULD NOT panic or hang on it: the config file, the state file, repo URLs, form posts, git HTTP requests, upstream responses and `git` output. "Never panics" is the invariant here (C8 does not apply). A new reader of outside input SHOULD get a test with damaged input (truncated, wrong type, junk) in the same change.

## Fixtures

- Fixtures SHOULD be generated by committed test code from our own inputs, as `tests/common/git.rs` and `tests/common/forge.rs` build repositories and a fake forge. A mock only where no real sample can be made.
- A committed file a test reads follows [licences.md](licences.md) and gets an `ATTRIBUTION.md` row unless it is our own work.
- A binary fixture SHOULD be under 100 KB.

## Tests that need a tool

A test that needs something the machine may lack (Chrome, `git`) SHOULD fail with a message naming what is missing and how to provide it, as `tests/common/browser.rs` does with `GIT_ARCHIVER_TEST_BROWSER`. It SHOULD NOT skip, so a run can't pass without exercising the code.

## Flaky and hanging tests

- A test that fails and then passes on a rerun of the same commit is flaky. Open an issue with the failing log. You SHOULD NOT rerun until it passes.
- If it failed on two unrelated pull requests, it MAY be quarantined with `#[ignore = "flaky: #<issue>"]` in a pull request of its own, until the issue is fixed.
- A test that fails every time MUST NOT be quarantined: fix the code or the test.
- A test that hangs is a failure.

## Examples of kept tests

- **`an_existing_config_is_never_overwritten`** (`tests/config.rs`). Guards: first start and restarts never clobber an operator's config. Breaks if: writing the default config stops checking for an existing file. `the_default_config_survives_a_restart_unchanged` would still pass, because it starts from the default.

## Cutting existing tests

- A cut SHOULD be its own pull request, one test file each, containing no other change.
- The pull request MUST list every removed or trimmed test, with its category, one line each, in the format above. A maintainer can then object to a single line instead of the whole cut.
- Judging a whole file SHOULD start from the code rather than the tests: list what in the code could plausibly break unnoticed, then map each test to one of those items or to a category.
