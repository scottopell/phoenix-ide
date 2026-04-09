---
created: 2025-07-05
priority: p1
status: done
artifact: src/runtime/executor.rs
---

# Sub-agents write files to main checkout instead of worktree

## Summary

When sub-agents are spawned from a Work conversation, they sometimes write files to the main git checkout instead of the parent's worktree. This pollutes the main checkout with untracked files.

## Context

Observed during bedrock Allium distillation (task 02638). Explore-mode sub-agents spawned for code reading created `BEDROCK_*.md` files in the main checkout (`/home/bits/dev/phoenix-ide/`) rather than the worktree. The sub-agents' working directory or system prompt may not communicate the worktree boundary clearly enough.

Files found on main checkout after sub-agent work:
```
?? BEDROCK_CONV_MODE_MAP.md
?? BEDROCK_INDEX.md
?? BEDROCK_MAP.md
?? BEDROCK_SUMMARY.md
```

These are Explore sub-agents (read-only), so file writes shouldn't happen at all. Two possible issues:
1. Sub-agent working directory set to main checkout instead of worktree
2. Sub-agent system prompt doesn't communicate worktree scope or read-only constraint
3. Explore sub-agents have write access they shouldn't (tool registry misconfiguration)

## Acceptance Criteria

- [ ] Explore sub-agents cannot create files in any directory
- [ ] Work sub-agents spawned from a Work parent write only within the parent's worktree
- [ ] Sub-agent system prompt includes worktree path and scope constraints
- [ ] Verify sub-agent working directory matches REQ-PROJ-008 (parent's worktree for Work parent, main checkout for Explore parent)

## Notes

REQ-PROJ-008 specifies: Work parent spawns sub-agent → sub-agent working directory = parent's worktree. Explore parent spawns sub-agent → working directory = main branch checkout. The bug may be in how the working directory is threaded to the sub-agent's ConvContext, or in the system prompt not mentioning the boundary.
