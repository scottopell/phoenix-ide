---
created: 2026-04-06
priority: p1
status: done
artifact: src/api/handlers.rs
---

# Direct mode escape hatch for git repos (REQ-PROJ-018)

## Summary

Currently impossible to get full tool access in a git repo without going
through the Explore -> propose_task -> approve -> Work ceremony. A one-line
config change shouldn't require a plan approval flow.

Spec is written in specs/projects/requirements.md (REQ-PROJ-018). Need to
implement: UI option at conversation creation to start in Direct mode for
git repos, bypassing the Explore/Work lifecycle entirely.

## Done when

- [ ] New conversation form offers "Start in Direct mode" for git dirs
- [ ] Direct mode conversation gets full tools (bash, patch, everything)
- [ ] No worktree, no branch, no task file created
- [ ] propose_task tool NOT available in Direct mode
- [ ] StateBar shows "Direct" pill
- [ ] Mode preview subtitle reflects the choice
