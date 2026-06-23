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
structurally impossible. `propose_task` is intercepted like submit_result and serves two
shapes by mode: in Explore it is the blocking Explore→Work gateway; in the writing modes
(Work, Branch, Direct-in-a-git-repo) it is a non-blocking **fork proposal** that spawns a
fully decoupled top-level Work conversation off the repository default branch (REQ-PROJ-033
through 036). It is withheld only from Direct-not-in-a-repo and from sub-agents. Tool
registry is configured by mode: Explore exposes read-only/planning tools plus `bash` only
when `nono` reports an enforceable OS sandbox that can block network; the sandboxed
bash can read broadly, while writes are limited to scratch, synthetic-home, and
platform-temp locations, network is blocked, and ambient credential variables are
stripped. `patch` is scoped
to the discovered taskmd directory so the agent can draft/revise a task file
(REQ-PROJ-003 and REQ-PROJ-037). Write tools are enabled in Work and Branch. Push is a regular bash command with no
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
| **REQ-PROJ-006:** Task Files as Versioned Living Contracts | ✅ Complete | taskmd 1.0 (filename is metadata, no frontmatter) is the default; agent drafts the file via `patch` in Explore; committed on the task branch, not main (work-lifecycle REQ-WL-002). A plain `.md` file (no taskmd metadata, no on-approve status rename, branch `task-{stem}-{conv-id8}`) works too, behind the `TaskSource` seam |
| **REQ-PROJ-007:** Work Mode Enables Writes Within the Worktree | ✅ Complete | Task 08603 (M3). upgrade_to_work_mode() |
| **REQ-PROJ-008:** Work Sub-Agents Inherit the Worktree | ✅ Complete | Mode parameter, model override, max_turns, one-writer constraint, MCP access, cwd-scoping guard all implemented; spec normative in `specs/subagents/subagents.allium`. Explore-search-restricted MCP subset stays deferred (see subagents executive.md). |
| **REQ-PROJ-009:** ~~Complete a Task (Squash Merge)~~ | Removed | Code deleted. Superseded by REQ-PROJ-027 (push branch, user merges via PR) |
| **REQ-PROJ-010:** Abandon a Conversation | Moved | Relocated to work-lifecycle REQ-WL-001 |
| **REQ-PROJ-011:** PR Status Is the Branch Health Indicator | Moved | Relocated to work-lifecycle REQ-WL-003 |
| **REQ-PROJ-012:** Provide propose_task Tool to Agents | 🟡 Partial | Explore gateway shipped; the writing-mode fork registry/interception path (REQ-PROJ-033/036) is spec-only |
| **REQ-PROJ-013:** Platform Capability Detection | ✅ Complete | Uses `nono::Sandbox::support_info()` plus network-block enforcement probing to decide whether top-level Explore can expose sandboxed bash |
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
| **REQ-PROJ-026:** Branch Mode Lifecycle (Push, Mark Merged, Abandon) | Moved | Relocated to work-lifecycle REQ-WL-001 (abandon) and REQ-WL-002 (mark-merged) |
| **REQ-PROJ-027:** Simplified Managed Completion (Push Branch) | Moved | Relocated to work-lifecycle REQ-WL-002 |
| **REQ-PROJ-028:** Managed Mode Worktree from First Message | ✅ Complete | Worktree created on first message with temp branch |
| **REQ-PROJ-029:** Branch Mode in the Mode Picker | ✅ Complete | Mode picker offers Direct, Managed, and Branch |
| **REQ-PROJ-030:** PR Feedback Freshness Indicator | Moved | Relocated to pr-association REQ-PRA-001 |
| **REQ-PROJ-031:** Agent-Facing PR Context Baseline | Moved | Relocated to pr-association REQ-PRA-002 |
| **REQ-PROJ-032:** Bounded PR Feedback Refresh | Moved | Relocated to pr-association REQ-PRA-003 |
| **REQ-PROJ-033:** Propose a Decoupled Task Fork from a Writing Mode | 📐 Spec only | Non-blocking `propose_task` in Work/Branch/Direct-in-git; snapshots the task and continues |
| **REQ-PROJ-034:** Approve a Fork Proposal — Spawn an Independent Conversation | 📐 Spec only | Async approval spawns a fresh top-level Work conversation cut from the repository default branch (`main_ref`), never the origin's `base_branch`/HEAD |
| **REQ-PROJ-035:** Fork Provenance and Decoupling Guarantees | 📐 Spec only | `spawned_from_conversation_id` breadcrumb; no lifecycle notifications; proposal bound to origin |
| **REQ-PROJ-036:** Fork-Eligible Mode Availability | 📐 Spec only | Writing-mode matrix; Direct gated on git repo; Explore keeps its parking gateway |
| **REQ-PROJ-037:** Request Changes — Promote a Fork Proposal to an Explore Refinement | 📐 Spec only (not implemented yet) | Third review action promotes a pending proposal into a fresh Explore conversation seeded with the brief + change note; refinement runs via the Explore propose/feedback loop, decoupled from the origin |

**Progress:** of 33 active requirements, 27 complete, 1 partial (REQ-PROJ-012 — its
Explore gateway ships but the writing-mode fork path is spec-only), and 5 (REQ-PROJ-033..037,
task forks + fork-proposal Request Changes) specified but not yet implemented.
REQ-PROJ-009 and -023 removed; REQ-PROJ-015 descoped; REQ-PROJ-016 superseded by
REQ-PROJ-018.

## Dependencies

- `specs/bedrock/` -- REQ-BED-027, REQ-BED-028, REQ-BED-029 (mode state, approval states)
- `specs/bash/` -- REQ-BASH-012, REQ-BASH-013 (Explore mode read-only bash enforcement)
- `specs/patch/` -- REQ-PATCH-009 (patch disabled in Explore mode)
- `specs/prose-feedback/` -- REQ-PF-015, REQ-PF-016 (programmatic task approval trigger)
