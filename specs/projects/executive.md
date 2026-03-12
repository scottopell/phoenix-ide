# Projects — Executive Summary

## Requirements Summary

The Projects feature gives PhoenixIDE a structured, git-backed workspace model. Every
conversation begins in Explore mode: read-only, pinned to the current main branch HEAD,
with no setup or risk. Users explore, ask questions, and plan freely. When an agent is
ready to make real changes, it proposes a task via `propose_plan`. The task file
(written to `tasks/` on main) is presented for human review using the prose reader.
Users can annotate the plan, request revisions, or approve. On approval, a dedicated
branch is created and the conversation enters Work mode. When work is complete, the user initiates a Complete action which squash merges the
task branch into the base branch with an LLM-generated commit message; alternatively,
the user can Abandon the task, which destructively discards the worktree and branch.
Both actions transition the conversation to Terminal state. Task files on main give
every conversation project-wide awareness of what is in-progress, planned, or done
without any special API.

## Technical Summary

`ConvMode` (Explore or Work) is a conversation-level field stored in SQLite alongside
the state machine state. `ConvMode::Work` includes `base_branch` recorded at approval
time. The state machine emits typed effects for git operations; the executor performs
them. One new state: `AwaitingTaskApproval` (task plan under human review). Task
completion and abandonment are user-initiated executor actions (no
`AwaitingMergeApproval` state). Complete does a squash merge into base_branch with
LLM-generated commit message; abandon destructively deletes worktree+branch. Both
transition to Terminal. Worktree paths are derived from conversation IDs -- collision
is structurally impossible. One new tool: `propose_plan` (Explore mode only, pure
data carrier intercepted like submit_result). During Work mode, agents update task
files via the standard patch tool. Tool registry is configured by mode: patch is
disabled in Explore, enabled in Work. Work sub-agents inherit the parent's worktree
and can optionally receive write access (one at a time). A passive poll-based
commits-behind indicator shows base branch advancement in the StateBar.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-----------|
| **REQ-PROJ-001:** Open a Git Repository as a Project | ✅ Complete | Task 0601 (M1) |
| **REQ-PROJ-002:** Start Every Conversation in Explore Mode | ✅ Complete | Task 0601 (M1) |
| **REQ-PROJ-003:** Propose a Task to Initiate Work Mode | ❌ Not Started | - |
| **REQ-PROJ-004:** Review and Iterate on Task Plan Before Starting Work | ❌ Not Started | - |
| **REQ-PROJ-005:** Worktree Paths Are Unique by Construction | ❌ Not Started | - |
| **REQ-PROJ-006:** Task Files as Versioned Living Contracts | ❌ Not Started | - |
| **REQ-PROJ-007:** Work Mode Enables Writes Within the Worktree | ❌ Not Started | - |
| **REQ-PROJ-008:** Work Sub-Agents Inherit the Worktree | ❌ Not Started | - |
| **REQ-PROJ-009:** Complete a Task (Squash Merge) | ❌ Not Started | User-initiated, squash merge to base_branch, Terminal state |
| **REQ-PROJ-010:** Abandon a Task (Destructive Discard) | ❌ Not Started | Delete worktree+branch, task to wont-do, Terminal state |
| **REQ-PROJ-011:** Passive Commits-Behind Indicator | ❌ Not Started | Poll-based, badge in StateBar, no rebase automation |
| **REQ-PROJ-012:** Provide propose_plan Tool to Agents | ❌ Not Started | Pure data carrier, intercepted like submit_result |
| **REQ-PROJ-013:** Platform Capability Detection | ✅ Complete | Task 0601 (M1) |
| **REQ-PROJ-014:** Project UI | ✅ Complete | Task 0601 (M1). Project tabs, mode badges |
| **REQ-PROJ-015:** Project Worktree Registry | Descoped | ConvMode::Work serves as de facto registry |
| **REQ-PROJ-016:** Standalone Conversation Mode | ✅ Complete | Task 0601 (M1). Non-git dirs get full tools, no project |
| **REQ-PROJ-017:** Base Branch Tracking in Work Mode | ❌ Not Started | ConvMode::Work stores base_branch from approval time |

**Progress:** 5 of 17 complete

## Known Gaps

- **Sidebar mode badge lag:** When conv_mode changes (e.g., Explore to Work on
  approve), the sidebar badge updates on the next 5-second poll, not instantly.
  Acceptable for M2. Real-time push from conversation atom to sidebar is a future
  optimization.

## Dependencies

- `specs/bedrock/` — REQ-BED-027, REQ-BED-028, REQ-BED-029 (mode state, approval states)
- `specs/bash/` — REQ-BASH-008, REQ-BASH-009 (Explore mode read-only enforcement)
- `specs/patch/` — REQ-PATCH-009 (patch disabled in Explore mode)
- `specs/prose-feedback/` — REQ-PF-015, REQ-PF-016 (programmatic task approval trigger)
