# Projects -- Executive Summary

## Requirements Summary

The Projects feature gives PhoenixIDE a structured, git-backed workspace model with
three conversation modes. Direct mode is the default for all conversations: full tool
access, no worktrees, no ceremony. Managed mode is opt-in for git repositories and
provides a two-phase lifecycle: conversations start in Explore (read-only worktree
created on first message), then upgrade to Work when the user approves a task proposed
via `propose_plan`. The plan is presented for human review; users can annotate, request
revisions, or approve. On approval, the temporary branch is renamed to the final task
branch, a task file is committed on that branch, and write tools are enabled. Branch
mode lets users work directly on an existing branch with no Explore phase and no task
file. A branch picker with local listing (sorted by recency, with staleness counts) and
on-demand remote search (cached `git ls-remote`) supports both Managed and Branch mode
branch selection. When work is complete, the agent pushes the branch to origin and the
user merges via PR on their hosting platform. The user then marks the conversation as
merged (terminal) or abandons it. In Managed mode, abandon deletes the worktree and
branch; in Branch mode, abandon deletes only the worktree, keeping the user's branch.

## Technical Summary

`ConvMode` has three variants: Direct, Explore, Work -- plus a distinct Branch mode
stored as a separate conversation-level field in SQLite. Direct carries no git metadata.
Managed conversations start in Explore: a worktree is created on first message using a
temporary branch (`task-pending-{id}`), with a best-effort single-branch fetch of the
base branch. On task approval, the temp branch is renamed to `task-{NNNN}-{slug}`, a
task file is committed on the task branch (not main), and the mode upgrades to Work.
Branch mode creates a worktree on the user's chosen branch immediately, with no Explore
phase and no task file. Worktree paths are derived from conversation IDs -- collision is
structurally impossible. One tool: `propose_plan` (Explore mode only, pure data carrier
intercepted like submit_result). Tool registry is configured by mode: write tools
disabled in Explore, enabled in Work and Branch. Push is a regular bash command with no
lifecycle side effects. Terminal actions are Mark as Merged and Abandon, both
user-initiated. Managed mode deletes the branch on terminal; Branch mode keeps it. A
remote-aware commits-behind poller does single-branch fetches on a 60-second interval
and emits SSE updates. Branch discovery uses local `git for-each-ref` for instant
listing and cached `git ls-remote` for on-demand remote search (5-minute TTL).

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-----------|
| **REQ-PROJ-001:** Open a Git Repository as a Project | ✅ Complete | Task 08601 (M1) |
| **REQ-PROJ-002:** Start Every Conversation in Explore Mode | ✅ Complete | Task 08601 (M1) |
| **REQ-PROJ-003:** Propose a Task to Initiate Work Mode | ✅ Complete | Task 08602 (M2). propose_plan tool |
| **REQ-PROJ-004:** Review and Iterate on Task Plan Before Starting Work | 🔄 Needs Update | Approval becomes permission upgrade in existing worktree (REQ-PROJ-028) |
| **REQ-PROJ-005:** Worktree Paths Are Unique by Construction | ✅ Complete | Task 08603 (M3). Derived from conversation UUID |
| **REQ-PROJ-006:** Task Files as Versioned Living Contracts | 🔄 Needs Update | Task file committed on branch, not main (REQ-PROJ-027) |
| **REQ-PROJ-007:** Work Mode Enables Writes Within the Worktree | ✅ Complete | Task 08603 (M3). upgrade_to_work_mode() |
| **REQ-PROJ-008:** Work Sub-Agents Inherit the Worktree | 🔄 Partial | Sub-agents work but missing: mode parameter (explore/work), model override, one-writer constraint, MCP access |
| **REQ-PROJ-009:** ~~Complete a Task (Squash Merge)~~ | Removed | Code deleted. Superseded by REQ-PROJ-027 (push branch, user merges via PR) |
| **REQ-PROJ-010:** Abandon a Conversation | 🔄 Needs Update | Branch mode keeps branch on abandon; Managed deletes it |
| **REQ-PROJ-011:** Passive Commits-Behind Indicator | ✅ Complete | Task 08604 (M4). StateBar badge |
| **REQ-PROJ-012:** Provide propose_plan Tool to Agents | ✅ Complete | Same as REQ-PROJ-003 |
| **REQ-PROJ-013:** Platform Capability Detection | ✅ Complete | Task 08601 (M1) |
| **REQ-PROJ-014:** Project UI | ✅ Complete | Task 08601 (M1). Project tabs, mode badges, Tasks panel |
| **REQ-PROJ-015:** Project Worktree Registry | Descoped | ConvMode::Work serves as de facto registry |
| **REQ-PROJ-016:** Standalone Conversation Mode | ✅ Complete | Task 08601 (M1). Non-git dirs get full tools, no project |
| **REQ-PROJ-017:** Base Branch Tracking in Work Mode | ✅ Complete | Task 08603 (M3). ConvMode::Work stores base_branch |
| **REQ-PROJ-018:** Direct Mode | ✅ Complete | Default for all conversations |
| **REQ-PROJ-019:** Conversation List Filtering | ✅ Complete | Mode/project filters, auto-archive |
| **REQ-PROJ-020:** Branch Discovery (Local, No Network) | ✅ Complete | Branch picker with search, staleness counts, recency sort |
| **REQ-PROJ-021:** Remote Branch Search (On-Demand) | ✅ Complete | Cached `git ls-remote` with 5-min TTL, substring filter |
| **REQ-PROJ-022:** Branch Materialization (Single-Branch Fetch) | ✅ Complete | Best-effort single-branch fetch at worktree creation |
| **REQ-PROJ-023:** Remote-Aware Commits-Behind Polling | ✅ Complete | 60s poller with single-branch fetch, SSE delta updates |
| **REQ-PROJ-024:** Work Directly on an Existing Branch (Branch Mode) | ✅ Complete | Worktree on existing branch, no task file, no Explore phase |
| **REQ-PROJ-025:** One Active Work Conversation Per Branch | ✅ Complete | Conflict detection with redirect/delete/fresh-start options |
| **REQ-PROJ-026:** Branch Mode Lifecycle (Push, Mark Merged, Abandon) | ✅ Complete | Push via bash; Mark as Merged and Abandon as terminal actions |
| **REQ-PROJ-027:** Simplified Managed Completion (Push Branch) | ✅ Complete | Push branch, user merges via PR; task file on branch, not main |
| **REQ-PROJ-028:** Managed Mode Worktree from First Message | ✅ Complete | Worktree created on first message with temp branch |
| **REQ-PROJ-029:** Branch Mode in the Mode Picker | ✅ Complete | Mode picker offers Direct, Managed, and Branch |

**Progress:** 23 of 27 complete (2 descoped, 1 partial, 3 needs update)

## Remaining Work

REQ-PROJ-008 (Work Sub-Agents) is the only incomplete pre-024 requirement. Needed:

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
