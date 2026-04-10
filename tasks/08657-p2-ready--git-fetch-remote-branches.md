---
created: 2026-04-10
priority: p2
status: ready
artifact: src/api/handlers.rs
---

# Git Fetch Remote Branches in Branch Picker

## Summary

Add a Fetch button to the managed conversation branch picker and include the remote's
default branch (e.g. `origin/main`) in the branch list so users can start a
conversation at the upstream tip without needing a local tracking branch.

## Context

The current `GET /api/git/branches` endpoint runs `git branch --list` which only
shows local branches. Remote branches at `origin/main` or any untracked remote branch
are invisible to the picker. Users working on a remote server currently have no way
to fetch without SSH-ing in.

This was filed as a follow-on to the terminal spec task (24657). The terminal
provides a general escape hatch for ad-hoc commands, but a one-click Fetch for the
branch picker is a high-value convenience that avoids opening a terminal just to run
`git fetch`.

## Acceptance Criteria

- [ ] `GET /api/git/branches` (or a new endpoint) runs `git fetch` before listing,
      or a separate `POST /api/git/fetch` endpoint is added
- [ ] Branch list includes remote-only branches (e.g. `origin/main`) so the user
      can select the upstream tip as the base branch for a new conversation
- [ ] A Fetch button is visible in the managed conversation branch picker UI
- [ ] Fetch button shows a loading state while the fetch is in progress
- [ ] Fetch errors are surfaced inline (not silently ignored)
- [ ] Local-only branches continue to appear as before
- [ ] `./dev.py check` passes

## Notes

- The remote default branch (e.g. `origin/main`) should be visually distinguished
  from local branches in the picker (e.g. prefixed with `origin/`).
- `git fetch --prune` is preferable to bare `git fetch` to keep the local remote-ref
  list clean.
- Consider whether to auto-fetch on branch picker open vs. manual button only.
  Manual button is safer (avoids slow startup on repos with large remotes).
