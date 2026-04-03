---
created: 2026-04-03
priority: p1
status: ready
artifact: src/system_prompt.rs
---

# Mode-aware system prompt

## Summary

The system prompt does not receive ConvMode. The agent in Explore mode has no
idea it's read-only, doesn't know propose_plan exists as a workflow tool, and
will try to use bash/patch on write requests -- hitting errors the user doesn't
understand.

## What to change

Wire ConvMode into `build_system_prompt`. Add mode-specific sections:

- **Explore**: "You are in Explore mode (read-only). You can read files, search,
  and analyze. To make changes, use propose_plan to propose a task. The user will
  review and approve before a writable workspace is created."
- **Work**: "You are in Work mode on branch {branch_name}. You have write access
  to the worktree. When done, the user will merge your changes to {base_branch}."
- **Standalone**: "You are in Direct mode with full tool access. This directory
  is not a git project."

## Done when

- [ ] `build_system_prompt` accepts ConvMode (or mode label + branch info)
- [ ] Explore prompt mentions read-only constraint and propose_plan workflow
- [ ] Work prompt mentions branch name and merge target
- [ ] Standalone prompt mentions full access
- [ ] Tests updated (use `build_system_prompt_with_home` to avoid $HOME contamination)
