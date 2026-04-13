---
created: 2026-04-12
priority: p3
status: done
artifact: pending
---

# seed-primitive-mode-auto-detection

## Problem

REQ-SEED-002 says callers specify `conv_mode` explicitly when spawning
a seeded conversation — no auto-detection. The rationale at the time
was that each caller knows what mode it wants:

- Shell integration assist → `direct` in `$HOME`
- Taskmd panel "start task" → `worktree` in the project root

This KISS'd the primitive to ship but pushes a minor annoyance onto
every future caller: they must hard-code a mode choice and justify it,
or invent their own heuristic.

An `auto` mode would inspect the target cwd at create time and pick:
- `worktree` if the target is inside a git repository AND the project
  has a worktree policy configured
- `direct` otherwise

This matches the heuristic Phoenix already uses elsewhere for new
conversations and lets callers opt in without duplication.

## Scope

- Add `auto` as a valid value for the seed payload's mode hint (only
  exposed if the primitive adds such a field — currently the mode is
  just passed through as `conv_mode` on the create request)
- Backend logic: when `auto` is passed, inspect the target cwd:
  - Walk up looking for `.git` (or use the existing Phoenix helper
    that already does this — there must be one for the non-seeded
    new-conversation flow)
  - If found, resolve to `worktree` mode
  - Otherwise `direct`
- Expose the resolved mode on the created conversation so the UI can
  render consistently
- Update REQ-SEED-002 to document the new behavior

## Why p3

Both existing callers already pick explicitly. No user-visible pain
until there's a third caller that would benefit. Keep in the backlog
until the taskmd panel (task 24669) ships or someone else asks for
it — at that point, if the third caller would also benefit, fold it
in.

## Out of scope

- Any more sophisticated mode heuristics (e.g. "worktree if the user
  has done a worktree session in this repo before"). KISS.
- Letting the user override the auto-detection at the UI level.
  That's a conversation-creation UX concern, not a seed primitive
  concern.

## Related

- Parent: 24666 (seed primitive v1)
- REQ-SEED-002 in `specs/seeded-conversations/requirements.md`
