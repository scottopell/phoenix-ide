# Projects — Executive Summary

## Requirements Summary

The Projects feature gives PhoenixIDE a structured, git-backed workspace model. Every
conversation begins in Explore mode: read-only, pinned to the current main branch HEAD,
with no setup or risk. Users explore, ask questions, and plan freely. When an agent is
ready to make real changes, it proposes a task via `create_task`. The task file
(written to `tasks/` on main) is presented for human review using the prose reader.
Users can annotate the plan, request revisions, or approve. On approval, a git worktree
is created for the conversation and work begins in isolation. Multiple conversations can
work on the same project simultaneously — each in its own worktree, no coordination
required. When work is complete, the agent signals ready-for-review and the user
approves the merge. Task files on main give every conversation project-wide awareness
of what is in-progress, planned, or done without any special API.

## Technical Summary

`ConvMode` (Explore or Work) is a conversation-level field stored in SQLite alongside
the state machine state. The state machine emits typed effects for git operations;
the executor performs them. Two new states: `AwaitingTaskApproval` (task plan under
human review) and `AwaitingMergeApproval` (diff under human review before merge).
Worktree paths are derived from conversation IDs — collision is structurally
impossible. Two new tools: `create_task` (Explore mode only) and `update_task` (Work
mode, parent conversations only). Tool registry is configured by mode: patch is
disabled in Explore, enabled in Work. Work sub-agents inherit the parent's worktree
and can optionally receive write access (one at a time). A filesystem watcher detects
main branch advancement and emits ambient SSE notifications.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-----------|
| **REQ-PROJ-001:** Open a Git Repository as a Project | ❌ Not Started | - |
| **REQ-PROJ-002:** Start Every Conversation in Explore Mode | ❌ Not Started | - |
| **REQ-PROJ-003:** Propose a Task to Initiate Work Mode | ❌ Not Started | - |
| **REQ-PROJ-004:** Review and Iterate on Task Plan Before Starting Work | ❌ Not Started | - |
| **REQ-PROJ-005:** Worktree Paths Are Unique by Construction | ❌ Not Started | - |
| **REQ-PROJ-006:** Task Files as Versioned Living Contracts | ❌ Not Started | - |
| **REQ-PROJ-007:** Work Mode Enables Writes Within the Worktree | ❌ Not Started | - |
| **REQ-PROJ-008:** Work Sub-Agents Inherit the Worktree | ❌ Not Started | - |
| **REQ-PROJ-009:** Complete a Task and Propose Merging to Main | ❌ Not Started | - |
| **REQ-PROJ-010:** Abandon a Task Without Merging | ❌ Not Started | - |
| **REQ-PROJ-011:** Ambient Awareness of Main Branch Advancement | ❌ Not Started | - |
| **REQ-PROJ-012:** Provide create_task and update_task Tools to Agents | ❌ Not Started | - |
| **REQ-PROJ-013:** Platform Capability Detection | ❌ Not Started | Probe sandbox at startup, adapt Explore tool set |
| **REQ-PROJ-014:** Project UI | ❌ Not Started | Project switcher tabs, mode indicators |
| **REQ-PROJ-015:** Project Worktree Registry | ❌ Not Started | Track worktrees, reconcile on startup |

**Progress:** 0 of 15 complete

## Dependencies

- `specs/bedrock/` — REQ-BED-027, REQ-BED-028, REQ-BED-029 (mode state, approval states)
- `specs/bash/` — REQ-BASH-008, REQ-BASH-009 (Explore mode read-only enforcement)
- `specs/patch/` — REQ-PATCH-009 (patch disabled in Explore mode)
- `specs/prose-feedback/` — REQ-PF-015, REQ-PF-016 (programmatic task approval trigger)
