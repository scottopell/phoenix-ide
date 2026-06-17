# Disallow filesystem root as conversation cwd

## Problem

A production sub-agent ran with effective `cwd: "/"`. The screenshot shows its relative tool paths resolving from `/` (`read_file .` attempted to read `/`, and `src/` resolved to `/src`). Static tracing shows Phoenix persists cwd values into conversations and uses them to build runtime working directories; if `/` reaches that seam, tools can run from the filesystem root.

The harm is not limited to write safety or privacy prompts. In the incident, `keyword_search` spawned `rg` rooted at `/`, causing unbounded filesystem traversal and high CPU consumption for many minutes. Read-only mode is still dangerous when the working directory is a system root.

This task is intentionally narrow: `/` is never a legitimate working directory for any Phoenix conversation or sub-agent, regardless of mode or parent mode.

## Proposed fix

1. Add a shared cwd validation/resolution seam used before a cwd is persisted or used to build a runtime context.
   - Reject `/` for all conversations and sub-agents, all modes.
   - Resolve symlinks and `..` before checking so aliases that resolve to `/` are rejected too.
   - Fail closed if validation cannot establish that the cwd is an acceptable directory.
   - Omitted sub-agent cwd continues inheriting the parent cwd unchanged, except inherited `/` must also be rejected at the same seam.

2. Apply the seam to both paths:
   - Top-level conversation creation / cwd updates that persist a conversation cwd.
   - Sub-agent spawn handling before creating the child conversation and before building its runtime.

3. Keep this task scoped to the universal root floor only.
   - Do not implement broader worktree containment here.
   - Do not change cancellation semantics here.

4. Add regression tests:
   - `/` rejected for a top-level conversation.
   - `/` rejected for a sub-agent of a Direct parent.
   - a symlink or `..` path that resolves to `/` is rejected.
   - a legitimate deep working directory is accepted.

5. Update specs to match the corrected invariant:
   - Root/system-root cwd is invalid for any conversation runtime because even read-only tools can cause unbounded resource consumption when rooted at `/`.
   - Keep wording timeless; do not describe this as an incident/changelog in specs.

6. Validate:
   - Targeted Rust tests around cwd validation and sub-agent spawn.
   - `./dev.py check` if time permits.

## Acceptance criteria

- No top-level conversation or sub-agent conversation can persist or run with cwd resolving to `/`.
- Direct-mode sub-agents are covered; the check is not gated on Work/Branch parent mode.
- Legitimate non-root directories continue to work.
- The task does not broaden into worktree containment or cancellation handling.
