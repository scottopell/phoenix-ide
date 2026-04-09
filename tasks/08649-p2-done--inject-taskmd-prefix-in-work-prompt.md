---
created: 2026-04-07
priority: p2
status: done
artifact: src/system_prompt.rs
---

# Include taskmd prefix in Work mode system prompt

## Problem

When a conversation transitions to Work mode, the system prompt tells the
agent its branch name and worktree path, but not the taskmd ID prefix for
the current worktree. The agent creates task files with `taskmd next` but
has no way to know the prefix pattern (e.g., "02638") that ties task files
to this worktree's namespace.

The whole point of taskmd's two-character base-36 prefix derived from the
directory path is to scope task IDs to a project/worktree. If the agent
doesn't know the prefix, it can't reason about which tasks belong to its
worktree vs the main checkout.

## What to change

In the Work mode system prompt injection (system_prompt.rs, ModeContext::Work),
include the taskmd prefix for the worktree path. Compute it via
`taskmd_core::ids::dir_prefix()` (or equivalent) from the worktree path
and add it to the prompt:

```
Your task ID prefix is {prefix}. Task files in this worktree use IDs
starting with {prefix} (e.g., {prefix}001, {prefix}002).
```

## Done when

- [ ] Work mode prompt includes the taskmd prefix
- [ ] Agent can reference its own task ID namespace in conversations
