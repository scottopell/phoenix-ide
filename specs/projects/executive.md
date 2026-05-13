# Projects -- Executive Summary

## Requirements Summary

The Projects feature gives PhoenixIDE a structured, git-backed workspace model with
three conversation modes. Direct mode is the default for all conversations: full tool
access, no worktrees, no ceremony. Managed mode is opt-in for git repositories and
provides a two-phase lifecycle: conversations start in Explore (read-only worktree
created on first message), then upgrade to Work when the user approves a task proposed
via `propose_task`. The plan is presented for human review; users can annotate, request
revisions, or approve. On approval, the temporary branch is renamed to the final task
branch, a task file is committed on that branch, and write tools are enabled. Branch
mode lets users work directly on an existing branch with no Explore phase and no task
file. A branch picker with local listing (sorted by recency, with staleness counts) and
on-demand remote search (cached `git ls-remote`) supports both Managed and Branch mode
branch selection. When work is complete, the agent pushes the branch to origin and the
user merges via PR on their hosting platform. Phoenix observes PR state through
GitHub CLI when available: merged PRs get a first-class cleanup affordance,
unmerged PRs are discouraged from cleanup, and manual "Mark as merged" remains a
fallback when PR state is unavailable. In Managed mode, abandon deletes the worktree and
branch; in Branch mode, abandon deletes only the worktree, keeping the user's branch.

## Technical Summary

`ConvMode` is a four-variant enum stored as a JSON column on the conversation: Direct
(default, no git metadata), Explore (`worktree_path: Option`), Work (`worktree_path`,
`branch_name`, `base_branch`, `task_id`, `task_title`), and Branch (`worktree_path`,
`branch_name`, `base_branch`; no `task_id`). Managed conversations start in Explore: a
worktree is created on first message using a temporary branch (`task-pending-{id}`),
with a best-effort single-branch fetch of the base branch. The agent drafts a task file
under the project's tasks directory with `patch`; on `propose_task` + approval the temp
branch is renamed to `task-{NNNN}-{slug}`, the task file's status is promoted to
`in-progress` and it is committed on that branch (never main), and the mode upgrades to
Work.
Branch mode creates a worktree on the user's chosen branch immediately, with no Explore
phase and no task file. Worktree paths are derived from conversation IDs -- collision is
structurally impossible. One tool: `propose_task` (Explore mode only, pure data carrier
intercepted like submit_result). Tool registry is configured by mode: write tools
disabled in Explore, enabled in Work and Branch. Push is a regular bash command with no
lifecycle side effects. Phoenix can observe PR state through `gh` to guide the
user-visible cleanup affordance, but does not push, merge, or run unattended
cleanup. Terminal actions remain user-initiated: verified merged PR cleanup / manual
Mark as Merged and Abandon. Managed mode deletes the branch on terminal; Branch mode keeps it.
Branch discovery uses local `git for-each-ref` for instant listing and cached `git ls-remote`
for on-demand remote search (5-minute TTL).

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-----------|
| **REQ-PROJ-001:** Open a Git Repository as a Project | ✅ Complete | Task 08601 (M1) |
| **REQ-PROJ-002:** Start Every Conversation in Explore Mode | ✅ Complete | Task 08601 (M1) |
| **REQ-PROJ-003:** Propose a Task to Initiate Work Mode | ✅ Complete | Task 08602 (M2). propose_task tool; task 13009 — `task_file` may be any `.md` file, taskmd naming is one accepted form (`crate::task_source::TaskSource`) |
| **REQ-PROJ-004:** Review and Iterate on Task Plan Before Starting Work | ✅ Complete | Approval is a permission upgrade in the existing worktree (REQ-PROJ-028): rename temp branch, promote+commit the agent's task file on it |
| **REQ-PROJ-005:** Worktree Paths Are Unique by Construction | ✅ Complete | Task 08603 (M3). Derived from conversation UUID |
| **REQ-PROJ-006:** Task Files as Versioned Living Contracts | ✅ Complete | taskmd 1.0 (filename is metadata, no frontmatter) is the default; agent drafts the file via `patch` in Explore; committed on the task branch, not main (REQ-PROJ-027). Task 13009 — a plain `.md` file (no taskmd metadata, no on-approve status rename, branch `task-{stem}-{conv-id8}`) works too, behind the `TaskSource` seam |
| **REQ-PROJ-007:** Work Mode Enables Writes Within the Worktree | ✅ Complete | Task 08603 (M3). upgrade_to_work_mode() |
| **REQ-PROJ-008:** Work Sub-Agents Inherit the Worktree | ✅ Complete | Mode parameter, model override, max_turns, one-writer constraint, MCP access, cwd-scoping guard all implemented; spec normative in `specs/subagents/subagents.allium`. Explore-search-restricted MCP subset stays deferred (see subagents executive.md). |
| **REQ-PROJ-009:** ~~Complete a Task (Squash Merge)~~ | Removed | Code deleted. Superseded by REQ-PROJ-027 (push branch, user merges via PR) |
| **REQ-PROJ-010:** Abandon a Conversation | ✅ Complete | Worktree removed; Managed deletes the task branch, Branch keeps it; diff snapshot captured as a system message first; no task-file edit |
| **REQ-PROJ-011:** PR Status Is the Branch Health Indicator | ✅ Complete | PR badge replaces ahead/behind StateBar noise |
| **REQ-PROJ-012:** Provide propose_task Tool to Agents | ✅ Complete | Same as REQ-PROJ-003 |
| **REQ-PROJ-013:** Platform Capability Detection | ✅ Complete | Task 08601 (M1) |
| **REQ-PROJ-014:** Project UI | ✅ Complete | Task 08601 (M1). Project tabs, mode badges, Tasks panel |
| **REQ-PROJ-015:** Project Worktree Registry | Descoped | ConvMode::Work serves as de facto registry |
| **REQ-PROJ-016:** Standalone Conversation Mode | ⏭️ Superseded | Superseded by REQ-PROJ-018 (Direct Mode). `Standalone` was folded into `Direct` via migration 001; the `ConvMode::Standalone` variant no longer exists |
| **REQ-PROJ-017:** Base Branch Tracking in Work Mode | ✅ Complete | Task 08603 (M3). ConvMode::Work stores base_branch |
| **REQ-PROJ-018:** Direct Mode | ✅ Complete | Default for all conversations |
| **REQ-PROJ-019:** Conversation List Filtering | ✅ Complete | Mode/project filters, auto-archive |
| **REQ-PROJ-020:** Branch Discovery (Local, No Network) | ✅ Complete | Branch picker with search, staleness counts, recency sort |
| **REQ-PROJ-021:** Remote Branch Search (On-Demand) | ✅ Complete | Cached `git ls-remote` with 5-min TTL, substring filter |
| **REQ-PROJ-022:** Branch Materialization (Single-Branch Fetch) | ✅ Complete | Best-effort single-branch fetch at worktree creation |
| **REQ-PROJ-023:** Reserved | Removed | Commits-behind polling removed; PR status is the branch health signal |
| **REQ-PROJ-024:** Work Directly on an Existing Branch (Branch Mode) | ✅ Complete | Worktree on existing branch, no task file, no Explore phase |
| **REQ-PROJ-025:** One Active Work Conversation Per Branch | ✅ Complete | Conflict detection with redirect/delete/fresh-start options |
| **REQ-PROJ-026:** Branch Mode Lifecycle (Push, Mark Merged, Abandon) | ✅ Complete | Push via bash; PR-aware cleanup guidance via gh; Abandon as terminal action |
| **REQ-PROJ-027:** Simplified Managed Completion (Push Branch) | ✅ Complete | Push branch, user merges via PR; gh observes merge state for cleanup; task file on branch, not main |
| **REQ-PROJ-028:** Managed Mode Worktree from First Message | ✅ Complete | Worktree created on first message with temp branch |
| **REQ-PROJ-029:** Branch Mode in the Mode Picker | ✅ Complete | Mode picker offers Direct, Managed, and Branch |

**Progress:** of the 25 active requirements, all 25 complete. REQ-PROJ-009
and -023 removed; REQ-PROJ-015 descoped; REQ-PROJ-016 superseded by
REQ-PROJ-018.

## Dependencies

- `specs/bedrock/` -- REQ-BED-027, REQ-BED-028, REQ-BED-029 (mode state, approval states)
- `specs/bash/` -- REQ-BASH-008, REQ-BASH-009 (Explore mode read-only enforcement)
- `specs/patch/` -- REQ-PATCH-009 (patch disabled in Explore mode)
- `specs/prose-feedback/` -- REQ-PF-015, REQ-PF-016 (programmatic task approval trigger)
