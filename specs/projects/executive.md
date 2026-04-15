# Projects -- Executive Summary

## Requirements Summary

The Projects feature gives PhoenixIDE a structured, git-backed workspace model. Every
conversation begins in Explore mode: read-only, pinned to the current main branch HEAD,
with no setup or risk. Users explore, ask questions, and plan freely. When an agent is
ready to make real changes, it proposes a task via `propose_plan`. The task file
(written to `tasks/` on main) is presented for human review using the prose reader.
Users can annotate the plan, request revisions, or approve. On approval, a dedicated
branch is created and the conversation enters Work mode. When work is complete, the user
initiates a Complete action which squash merges the task branch into the base branch with
an LLM-generated commit message; alternatively, the user can Abandon the task, which
destructively discards the worktree and branch. Both actions transition the conversation
to Terminal state. Task files on main give every conversation project-wide awareness of
what is in-progress, planned, or done without any special API.

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
| **REQ-PROJ-001:** Open a Git Repository as a Project | ✅ Complete | Task 08601 (M1) |
| **REQ-PROJ-002:** Start Every Conversation in Explore Mode | ✅ Complete | Task 08601 (M1) |
| **REQ-PROJ-003:** Propose a Task to Initiate Work Mode | ✅ Complete | Task 08602 (M2). propose_plan tool |
| **REQ-PROJ-004:** Review and Iterate on Task Plan Before Starting Work | ✅ Complete | Task 08602 (M2). TaskApprovalReader + prose feedback |
| **REQ-PROJ-005:** Worktree Paths Are Unique by Construction | ✅ Complete | Task 08603 (M3). Derived from conversation UUID |
| **REQ-PROJ-006:** Task Files as Versioned Living Contracts | ✅ Complete | Task 08602 (M2). taskmd-core integration |
| **REQ-PROJ-007:** Work Mode Enables Writes Within the Worktree | ✅ Complete | Task 08603 (M3). upgrade_to_work_mode() |
| **REQ-PROJ-008:** Work Sub-Agents Inherit the Worktree | 🔄 Partial | Sub-agents work but missing: mode parameter (explore/work), model override, one-writer constraint, MCP access |
| **REQ-PROJ-009:** Complete a Task (Squash Merge) | ✅ Complete | Task 08604 (M4). Auto-stash support added |
| **REQ-PROJ-010:** Abandon a Task (Destructive Discard) | ✅ Complete | Task 08604 (M4). Worktree+branch deleted, task wont-do |
| **REQ-PROJ-011:** Passive Commits-Behind Indicator | ✅ Complete | Task 08604 (M4). StateBar badge |
| **REQ-PROJ-012:** Provide propose_plan Tool to Agents | ✅ Complete | Same as REQ-PROJ-003 |
| **REQ-PROJ-013:** Platform Capability Detection | ✅ Complete | Task 08601 (M1) |
| **REQ-PROJ-014:** Project UI | ✅ Complete | Task 08601 (M1). Project tabs, mode badges, Tasks panel |
| **REQ-PROJ-015:** Project Worktree Registry | Descoped | ConvMode::Work serves as de facto registry |
| **REQ-PROJ-016:** Standalone Conversation Mode | ✅ Complete | Task 08601 (M1). Non-git dirs get full tools, no project |
| **REQ-PROJ-017:** Base Branch Tracking in Work Mode | ✅ Complete | Task 08603 (M3). ConvMode::Work stores base_branch |
| **REQ-PROJ-018:** Direct Mode | ✅ Complete | Default for all conversations |
| **REQ-PROJ-019:** Conversation List Filtering | ✅ Complete | Mode/project filters, auto-archive |
| **REQ-PROJ-020:** Branch Discovery (Local, No Network) | 🔧 In Progress | Local branches sorted by recency, staleness signal |
| **REQ-PROJ-021:** Remote Branch Search (On-Demand) | 🔧 In Progress | `git ls-remote` with caching, substring search |
| **REQ-PROJ-022:** Branch Materialization (Single-Branch Fetch) | 🔧 In Progress | Auto-fetch selected branch at worktree creation |
| **REQ-PROJ-023:** Remote-Aware Commits-Behind Polling | 🔧 In Progress | Single-branch fetch in poller |

**Progress:** 15 of 21 complete (1 descoped, 1 partial, 4 in progress)

## Remaining Work

REQ-PROJ-008 (Work Sub-Agents) is the only incomplete requirement. Needed:

1. **Agent mode parameter** on spawn_agents: `mode: "explore" | "work"`. Explore gets
   read-only tools + cheaper model. Work gets full tools + parent model.
2. **One-writer constraint**: Only one Work sub-agent per parent at a time. Multiple
   Explore sub-agents allowed in parallel.
3. **MCP access**: Explore sub-agents get search-oriented MCP tools (deferred). Work
   sub-agents get the full MCP set.
4. **Model selection**: Explore defaults to haiku. Work inherits parent. Optional
   override per task.
5. **Max turns limit**: Per-agent turn cap (replaces or supplements 5-minute timeout).

## Dependencies

- `specs/bedrock/` -- REQ-BED-027, REQ-BED-028, REQ-BED-029 (mode state, approval states)
- `specs/bash/` -- REQ-BASH-008, REQ-BASH-009 (Explore mode read-only enforcement)
- `specs/patch/` -- REQ-PATCH-009 (patch disabled in Explore mode)
- `specs/prose-feedback/` -- REQ-PF-015, REQ-PF-016 (programmatic task approval trigger)
