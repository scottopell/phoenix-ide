---
created: 2026-04-12
priority: p2
status: done
artifact: pending
---

# fix-merge-diff-summarization-scope

## Problem

The "merge to main" diff summarization produced the wrong summary for task
24661 (React perf fixes). It summarised terminal, credential helper, and
release infrastructure instead of the actual perf changes.

**Reproduction:** branch `task-24661-fix-react-perf-antipatterns-conversation`
had these commits ahead of `origin/main`:

```
fc47958 chore: remove old in-progress task file
c5db670 chore: mark task 24661 as done
21ae255 fix: clippy useless_conversion in spawn.rs ...
c0affcf perf: fix React antipatterns causing keystroke sluggishness
2a84993 task 24661: fix-react-perf-antipatterns-conversation-view   <- task file commit
db19246 task 24660: terminal-implementation                         <- stale task file commit from rebase
```

The bottom two are task-file-only commits carried along during `git rebase
origin/main`. `db19246` re-adds the terminal task file (179 lines of terminal
implementation notes) that main had already absorbed. The summariser ingested
all 6 commits and produced a summary dominated by the terminal content.

## Root Cause

When a branch is rebased onto main, prior task-file commits from the worktree
branch history are replayed even though main has already incorporated those
tasks. The merge diff summariser uses `git log origin/main..HEAD` (all commits
on the branch) rather than the file-level diff, so noise commits inflate and
mislead the summary.

## Fix Options

1. **Switch to file-level diff:** Summarise `git diff origin/main...HEAD`
   (triple-dot — common ancestor to tip) instead of aggregating commit
   messages. This reflects exactly what files changed, regardless of commit
   history noise.

2. **Filter task-file-only commits:** Before summarising, exclude any commit
   where every changed file is under `tasks/`. These are bookkeeping, not
   feature work.

3. **Both:** Use the file diff as the primary input; optionally include commit
   messages only for commits that touch non-`tasks/` files.

Option 1 is simplest and most robust — the file diff is always the right
answer for "what does this branch actually change".

## Acceptance Criteria

- [ ] Merge diff summarisation uses file-level diff (`git diff origin/main...HEAD`
  or equivalent) as the primary input, not raw commit log
- [ ] Summary for a branch that only touches UI perf files does not mention
  unrelated features (terminal, credential helper, etc.)
- [ ] Summary for a branch with task-file-only commits does not include task
  file content as feature work
