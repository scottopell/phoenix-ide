---
created: 2026-05-23
priority: p3
status: ready
artifact: crates/phoenix-ide/src/api/handlers.rs
---

# Tighten run_resource_cleanup_cascade docstring re: log levels

## What

The docstring on `run_resource_cleanup_cascade` in
`crates/phoenix-ide/src/api/handlers.rs` says:

> All failures log WARN and continue

Not quite true. Inside `cascade_projects_on_delete`, the post-worktree
best-effort `git branch -D` fallback logs at `debug` (intentional —
branch deletion is best-effort) and does NOT populate
`project_report.error`. Only worktree-removal failures populate the
error field and log WARN at the orchestrator level.

## Why p3

Pure docstring drift. No behavioural bug; the operator-visible logs
still surface the things they need to act on. Copilot flagged it
during PR #135 review. Worth fixing so the docstring doesn't lie, but
not blocking anything.

## Fix

Reword the policy statement on `run_resource_cleanup_cascade`:

```
/// Worktree-removal, bash kill, tmux kill failures log WARN with the
/// fields needed for manual cleanup. Best-effort branch deletion
/// failures log at DEBUG and are not surfaced via the per-cascade
/// report struct — branch cleanup is opportunistic, not authoritative.
/// Callers own the final DB write and any state-machine transition.
```

Optionally: rename `project_report.error` to make the
worktree-vs-branch distinction explicit (e.g. `worktree_error`).

## Validation

- `./dev.py check` clean.
- Read the updated docstring against the actual cascade code paths,
  confirm every log call matches.

## Context

PR #135 Copilot review comment (#3293729973). Captured separately so
the archive-is-terminal commit on #135 stays focused.
