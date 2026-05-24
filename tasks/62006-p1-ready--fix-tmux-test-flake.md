---
created: 2026-05-23
priority: p1
status: ready
artifact: crates/phoenix-ide/src/tools/tmux.rs
---

# Fix flake: `tmux::tests::fresh_session_starts_in_supplied_cwd`

## What

`tools::tmux::tests::fresh_session_starts_in_supplied_cwd` failed once
during a `./dev.py check` run (cargo-nextest, 9 parallel test threads):

```
assertion `left == right` failed: pane should start in
"/private/var/folders/2k/_fb1709s6hggh7nlym0wb4l40000gn/T/.tmpvOFIaQ",
got ""
  left: ""
 right: "/private/var/folders/2k/_fb1709s6hggh7nlym0wb4l40000gn/T/.tmpvOFIaQ"
```

Re-ran the same suite immediately, all 17 lanes green. Test passes 100%
when run individually. Failure mode: `TmuxTool::run` returned successfully
(`is_success()` was true) but `stdout` was empty when the test asked tmux
for `#{pane_current_path}`.

Most likely cause: a race in the `ensure_live` -> `display-message`
sequence under parallel-load. The freshly-spawned `main` session may not
have a pane in steady state when `display-message -p` runs, so the
command returns empty output instead of the cwd. Other plausible
contributors: tmux subprocess fork/exec contention with 9 concurrent
tmux child processes, slow socket-creation on macOS under load.

## Why p1

Zero-tolerance policy on flakes. Even single-occurrence flakes accumulate
into "is CI red because of my change or just the usual?" friction that
erodes trust in the suite. The fix is small and contained.

## Fix direction

Two reasonable options:

1. **Wait for pane ready inside the test.** After `ensure_live` returns,
   poll `tmux list-panes -t main` (or `display-message -p '#{session_attached}'`)
   until a pane exists, then issue the assertion. Bounds the race to
   a deterministic check rather than relying on subprocess-spawn timing.

2. **Tighten `ensure_live` itself.** If `spawn_session` returns before
   the `main` session has a usable pane, that's a per-process bug
   masquerading as a test flake. Investigate whether `new-session -d` is
   truly synchronous w.r.t. pane creation on macOS, and add a probe loop
   inside `spawn_session` if not.

Prefer (2) if reproducible — the contract `ensure_live = panes usable`
should hold for all callers, not just this test. Use (1) as a fallback
if (2) can't be made deterministic.

## Validation

- Run `./dev.py check` in a loop (10+ iterations) on the fix branch.
  No `fresh_session_starts_in_supplied_cwd` failures across all runs.
- If pursuing fix direction 2, add a unit test that calls
  `ensure_live` followed immediately by `tmux list-panes` and asserts
  exactly one pane exists.

## Context

Surfaced during PR #136 / #139 stack rebase verification on
2026-05-23. Stack:
- #136 feat: introduce WorkScope; key tmux server by work scope
- #135 fix: unify lifecycle resource cleanup
- #139 feat(browser): key sessions by WorkScope; cascade integration

Flake is pre-existing — none of those PRs touch `ensure_live` /
`spawn_session` / the `TmuxTool::run` path that produced the empty
output. The test was added with the original tmux integration. Just
finally observed it under load.

Run logs:
- Failed run captured in the rebase-verification `./dev.py check` log
  (the `FAIL [   0.383s]` line for `fresh_session_starts_in_supplied_cwd`).
- Successful re-run immediately after: all 17 checks passed in 55.8s.
