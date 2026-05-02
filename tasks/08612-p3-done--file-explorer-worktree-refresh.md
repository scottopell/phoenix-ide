---
created: 2026-03-13
priority: p3
status: done
artifact: pending
---

# File explorer should refresh when CWD changes

## Problem

When a conversation transitions from Explore to Work mode (task approval),
the CWD changes from the project root to the worktree path. The file explorer
doesn't refresh to reflect the new directory. Observed in QA: the file
explorer sometimes showed the root filesystem (/) instead of the worktree
contents until a page reload.

Similarly, after Complete/Abandon, the CWD reverts but the file explorer
may be stale.

## What to Do

The file explorer derives its root from `conversation.cwd`. When the
`sse_conversation_update` event changes `cwd`, the file explorer should
re-fetch its directory listing.

Check how the file explorer gets its root path -- if it reads from
`conversation.cwd` reactively, it may just need a key change to force
re-render. If it caches the initial CWD, it needs to watch for updates.

## Acceptance Criteria

- [ ] File explorer updates immediately when CWD changes via SSE
- [ ] After task approval: shows worktree contents
- [ ] After complete/abandon: shows project root contents
- [ ] No stale filesystem listing shown
