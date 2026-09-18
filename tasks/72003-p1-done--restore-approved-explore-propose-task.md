# Restore `propose_task` for approved Explore write authority

## Observed journey

- A Git-backed Explore conversation completed an approved-task flow and retained the same conversation/worktree with write authority.
- On runtime reconstruction, its tool catalog switched to the full writing registry but omitted `propose_task`, so the authorized design conversation could not submit its implementation brief for blocking review.
- This is source repair only. Do not mutate the stranded conversation, lifecycle rows, production, deployment state, or unrelated subscription work.

## Verified findings

- `specs/bedrock/requirements.md` REQ-PROJ-033 and REQ-PROJ-036 require `propose_task` for both Git-backed planning/read-only conversations and Git-backed conversations with write authority, using the same blocking approval path. Direct conversations and sub-agents remain excluded.
- `specs/bedrock/bedrock.allium` defines one `ProposeTaskIntercepted` path for an eligible Git-backed parent and requires it to park in `awaiting_task_approval`; `UserApprovesTaskCurrentConversation` preserves the same conversation, mode, and attached work scope while persisting typed approved-task authority.
- Current `origin/main` `RuntimeManager` registry selection loads `approved_task_objective`, then handles `ConvMode::Explore` with an approved objective by constructing `ToolRegistry::direct(...).try_with_writing_conversation_tools(...)` without `.with_propose_task()`. Ordinary Explore and the writing-mode branches do include the tool through their respective constructors.
- The state-machine reducer still recognizes an Explore `ModeContext` as the blocking path and transitions a valid sole `propose_task` call to `AwaitingTaskApproval`; existing tests cover ordinary Explore but do not name the approved-Explore/write-authority regression.
- Existing registry matrix coverage proves base Direct and both sub-agent registries omit `propose_task`, but it does not exercise the `RuntimeManager` approved-Explore selection seam that regressed.
- Issue #651 currently lists “Explicit Coordinator conversation subscriptions — Implementation authorized; proposal gateway repair required” and says the heavy slot is occupied. This task must remain independent of that feature and defer heavy validation accordingly.
- The isolated checkout started behind current `origin/main`; implementation must begin by reconciling onto current main without merging unrelated active workstreams.

## Inferences and unknowns

- Failure model: registry selection treats approved Explore write authority as a generic writing registry and silently drops the proposal capability that Git-backed parent conversations must retain. The reducer path itself appears capable of blocking correctly once the tool is admitted.
- No product decision is needed: requirements and Allium already settle availability, blocking semantics, and Direct/sub-agent exclusions.

## Interaction map

`approved_task_objective` persistence → `RuntimeManager` reconstructs an approved Explore parent → parent tool registry advertises writing tools plus `propose_task` → sole valid tool call reaches the parent LLM-response interceptor → `AwaitingTaskApproval` is persisted and ordinary execution pauses → user approval/request-changes/reject follows the existing task-approval flow.

Direct/chat-only and sub-agent registry constructors remain outside this admission path.

## Proposed scope

1. In `crates/phoenix-ide/src/runtime.rs`, make the approved-Explore/write-authority registry composition structurally include `propose_task` alongside the existing full writing tools. Keep the change at the registry-construction seam; do not special-case a conversation ID or edit persisted rows.
2. Add focused runtime/registry regression coverage for the exact capability matrix:
   - Git-backed approved Explore parent with write authority advertises `propose_task` and writing tools.
   - Direct remains excluded from this repair.
   - Explore and Work sub-agents remain excluded.
   Prefer a small testable registry-selection helper only if needed to exercise the production branch; avoid a broad registry redesign.
3. Add focused `phoenix-state-machine` regression coverage showing that the approved-Explore shape’s sole valid `propose_task` call takes the existing blocking path: `AwaitingTaskApproval`, checkpoint/state persistence, no fork-proposal effect, and no immediate Git/lifecycle side effect.
4. Correct only nearby stale comments that would otherwise describe the repaired branch incorrectly. The normative bedrock requirements/Allium already state the intended behavior; do not rewrite specs unless implementation reveals a genuine contradiction.

## Acceptance evidence

- A focused registry test fails before the repair because the approved-Explore production selection omits `propose_task`, then passes after the repair.
- A reducer regression proves the same approved-Explore conversation parks in `AwaitingTaskApproval` and does not enter a nonblocking/fork path.
- Existing capability-boundary tests continue proving no `propose_task` for Direct or sub-agents.
- Run only focused lightweight tests while the devmbp heavy slot is occupied. Defer `./dev.py check`, builds, merges, deploys, and any heavy validation until the slot is explicitly available; record deferred validation accurately.

## Risks and non-goals

- Do not edit lifecycle/approval/database rows, recover or patch the stranded conversation directly, or add migration/compatibility behavior.
- Do not broaden `propose_task` to Direct, coordinator, or sub-agent registries.
- Do not implement or modify conversation subscriptions, proposal placement, fork workflows, approval persistence, UI, creation flows, branch/worktree lifecycle, or production state.
- Do not build, merge, deploy, restart production, or consume the occupied heavy slot as part of this task without later explicit availability.
