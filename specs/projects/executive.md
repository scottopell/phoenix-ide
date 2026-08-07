# Projects -- Executive Summary

## Requirements Summary

Projects covers how Phoenix detects Git-backed directories, provisions repository-backed conversations, exposes branch discovery, and presents current repository/worktree context in the UI. It also remains the main compatibility home for the shipped mode-based product model while unified Open/History lifecycle work is still spec-first.

## Current Reality

The shipped product still uses mode-driven conversation creation and execution. Git-backed repos can enter Direct, Managed, or Branch flows in the new-conversation composer and sidebar/context surfaces, with Managed splitting into Explore then Work after task approval. Worktrees are still created from branch-oriented flows, task approval still upgrades mode in-place for Continue-here behavior, and writing-mode `propose_task` follow-on placement is not yet the unified fresh-derived conversation flow. Branches and PRs are still heavily surfaced in product UI even though the new normative model treats them as observed repository facts rather than conversation-owned lifecycle state.

Concrete shipped anchors include `ConvMode` persistence and reconstruction in `crates/phoenix-db/src/lib.rs:8147-8255`, mode-specific transition/property tests in `crates/phoenix-state-machine/src/project_proptests.rs`, mode-aware routing in `ui/src/components/ConversationSettings.tsx`, and mode badges/labels in `ui/src/utils/conversationIdentity.ts` and `ui/src/components/ConversationList.tsx`.

## Technical Summary

Phoenix persists mode-derived Git context through `cm_*` conversation columns reconstructed into `ConvMode`, provisions/continues repository-backed conversations through the DB/runtime path, and exposes branch discovery plus project suggestions through the HTTP API and React composer/settings surfaces. Chain-era compatibility data such as `chain_name` and `continued_in_conv_id` still exists because the current UI still exposes chain and archived/current-era projections.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-----------|
| **REQ-PROJ-001:** Open a Git Repository as a Project | ✅ Complete (legacy current reality) | Git-backed directories resolve to projects and still enter the shipped mode-based creation flow |
| **REQ-PROJ-001A:** Suggest Known Projects for New Conversations | ✅ Complete | `/new` consumes `/api/projects`, ranks by active conversation count then project recency |
| **REQ-PROJ-002:** Default Conversation Mode Selection | ✅ Complete (legacy current reality) | Direct/Managed/Branch selection is still exposed in the composer and settings UI |
| **REQ-PROJ-003:** Propose a Task to Initiate Work Mode | ✅ Complete (legacy current reality) | `propose_task` still acts as the Explore→Work gateway in the existing worktree |
| **REQ-PROJ-004:** Review and Iterate on Task Plan Before Starting Work | ✅ Complete (legacy current reality) | Approval still upgrades the same conversation/worktree into Work mode rather than offering the unified placement model |
| **REQ-PROJ-005:** Worktree Paths Are Unique by Construction | ✅ Complete | Worktree paths remain conversation-derived under `.phoenix/worktrees/` |
| **REQ-PROJ-006:** Task Files as Versioned Living Contracts | ✅ Complete | taskmd/plain markdown proposal sources are still supported and persisted |
| **REQ-PROJ-007:** Work Mode Enables Writes Within the Worktree | ✅ Complete (legacy current reality) | Write authority remains mode-gated to Work / Branch |
| **REQ-PROJ-008:** Work Sub-Agents Inherit the Worktree | ✅ Complete | Shipped sub-agent execution still inherits parent worktree/scope |
| **REQ-PROJ-009:** ~~Complete a Task (Squash Merge)~~ | Removed | Code deleted |
| **REQ-PROJ-010:** Abandon a Conversation | Moved | Legacy implementation lives under `work-lifecycle`; current user surface still uses `/abandon-task` |
| **REQ-PROJ-011:** PR Status Is the Branch Health Indicator | Moved | See `pr-association` / `work-lifecycle` |
| **REQ-PROJ-012:** Provide propose_task Tool to Agents | 🟡 Partial | Tool is exposed in shipped eligible modes, but the unified approval-placement model is not implemented |
| **REQ-PROJ-013:** Platform Capability Detection | ✅ Complete | Explore-mode sandbox gating remains shipped |
| **REQ-PROJ-014:** Project UI | ✅ Complete (legacy current reality) | UI still exposes mode badges, branch/base/task context, and branch-oriented entry points |
| **REQ-PROJ-015:** Project Worktree Registry | Descoped | Persisted work-scope/conversation ownership is the effective authority |
| **REQ-PROJ-016:** Standalone Conversation Mode | ⏭️ Superseded | Superseded by Direct |
| **REQ-PROJ-017:** Base Branch Tracking in Work Mode | ✅ Complete (legacy current reality) | Work/Branch still persist base-branch metadata |
| **REQ-PROJ-018:** Direct Mode | ✅ Complete (legacy current reality) | Direct remains the default shipped mode |
| **REQ-PROJ-019:** Conversation List Filtering | ✅ Complete (legacy current reality) | Sidebar still filters around active/archived and project groupings, not Open/History aggregates |
| **REQ-PROJ-020:** Branch Discovery (Local, No Network) | ✅ Complete | Branch picker remains shipped |
| **REQ-PROJ-021:** Remote Branch Search (On-Demand) | ✅ Complete | Cached `git ls-remote` search remains shipped |
| **REQ-PROJ-022:** Branch Materialization (Single-Branch Fetch) | ✅ Complete | Existing worktree provisioning still does bounded branch fetch/materialization |
| **REQ-PROJ-023:** Reserved | Removed | Removed |
| **REQ-PROJ-024:** Work Directly on an Existing Branch (Branch Mode) | ✅ Complete (legacy current reality) | Branch mode remains a shipped product path |
| **REQ-PROJ-025:** One Active Work Conversation Per Branch | ✅ Complete (legacy current reality) | Current branch/worktree conflict handling still enforces the pre-unification invariant |
| **REQ-PROJ-026:** Branch Mode Lifecycle (Push, Mark Merged, Abandon) | Moved | Current UX still exposes legacy mark-merged / abandon via sibling lifecycle surfaces |
| **REQ-PROJ-027:** Simplified Managed Completion (Push Branch) | Moved | See `work-lifecycle`; current product still couples completion to legacy terminal actions |
| **REQ-PROJ-028:** Managed Mode Worktree from First Message | ✅ Complete (legacy current reality) | Managed still provisions on first message with the shipped branch-oriented flow |
| **REQ-PROJ-029:** Branch Mode in the Mode Picker | ✅ Complete (legacy current reality) | Mode picker still offers Direct / Managed / Branch |
| **REQ-PROJ-030:** PR Feedback Freshness Indicator | Moved | See `pr-association` |
| **REQ-PROJ-031:** Agent-Facing PR Context Baseline | Moved | See `pr-association` |
| **REQ-PROJ-032:** Bounded PR Feedback Refresh | Moved | See `pr-association` |
| **REQ-PROJ-033:** Propose a Decoupled Task Fork from a Writing Mode | 📐 Spec only | Unified fresh-derived writing-mode follow-on flow is not shipped |
| **REQ-PROJ-034:** Approve a Fork Proposal — Spawn an Independent Conversation | 📐 Spec only | No shipped approval path yet spawns the new normalized independent conversation flow |
| **REQ-PROJ-035:** Fork Provenance and Decoupling Guarantees | 📐 Spec only | Typed derived provenance is not yet the product behavior |
| **REQ-PROJ-036:** Fork-Eligible Mode Availability | 📐 Spec only | Writing-mode fork eligibility remains unimplemented |
| **REQ-PROJ-037:** Request Changes — Promote a Fork Proposal to an Explore Refinement | 📐 Spec only | Not implemented |
| **REQ-PROJ-038:** Show the Live Worktree Checkout in Diff Review | ✅ Complete | Diff/review still reads live checkout state rather than persisted assumptions |

## Verification Notes

Current-reality checks for this reconciliation used code anchors rather than normative speculation:

- `crates/phoenix-db/src/lib.rs` for `ConvMode`, continuation wiring, archived/current listing, and chain metadata
- `crates/phoenix-state-machine/src/project_proptests.rs` and `transition.rs` for mode transitions and task approval/terminal flows
- `ui/src/components/ConversationSettings.tsx`, `ui/src/components/ConversationList.tsx`, and `ui/src/utils/conversationIdentity.ts` for mode picker and mode-badge UX

## Dependencies

- `specs/bedrock/` — reducer/runtime lifecycle and approval states still underpin the current mode-based behavior
- `specs/work-lifecycle/` — current mark-merged / abandon surfaces live there while Close→History remains unimplemented
- `specs/pr-association/` — PR observation, freshness, and active-selection behavior
- `specs/work-scope-ui/` — resource visibility for the attached work scope
- `specs/chains/` — chain-era compatibility surface still depends on continuation topology retained here
