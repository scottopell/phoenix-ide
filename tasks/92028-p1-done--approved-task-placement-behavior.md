Child of task 92018.

# Approved-task placement behavior across bedrock + projects

## Objective
Specify the exact approved-task placement behavior across the bedrock and projects spec cluster, limited to how approved work resumes in place versus starts in a separate conversation.

## Exact target artifacts
- `specs/bedrock/bedrock.allium`
- `specs/bedrock/requirements.md` only if a normative clarification is strictly required to match the settled facts already accepted in 92017
- `specs/projects/requirements.md`
- any existing `specs/projects/*.allium` files if they already own approved-task placement behavior
- read-only context from `tasks/92017-p1-done--terminology-authority-requirements-basel.md`
- read-only context from `tasks/92018-p1-in-progress--lifecycle-close-allium-behavior.md`

## Settled facts from 92017 / 92018 that this task MUST encode
- `Continue here` resolves approved-task review and resumes the same conversation context.
- `Continue here` has no lifecycle, provenance, `WorkScope`, worktree, branch, or PR side effect.
- `Start in new conversation` creates a separate Open conversation with fresh `WorkScope`/worktree, exact approved task only, and visible Derived from provenance.
- Follow-up is separate and fresh rather than a continuation of the origin.
- This task owns approved-task placement behavior only; it must not absorb root lifecycle topology, durable Close orchestration, WorkScope retirement classification, or seamless History/SSE projection.

## Required work contract
- Keep the contract focused on approved-task placement across bedrock + projects.
- Encode only settled placement behavior already grounded by 92017/92018 facts and current requirements/code reality.
- Do **not** add speculative review states, guessed UI flows, placeholder provenance helpers, or unrelated lifecycle/resource obligations.
- Run `allium check` on every edited `.allium` file and record exact commands/results in the evidence ledger.

## Out of scope
- `specs/durable-workflows/**`
- `specs/work-lifecycle/**`
- `specs/pr-association/**`
- `specs/sse_wire/**`
- code, ADRs, executive docs

## Evidence ledger required before marking done
Append a completion note with these headings:
- **Files changed** — exact paths changed
- **Settled facts encoded** — bullets mapping the task facts above to concrete rules/requirements
- **Validation** — exact `allium check` commands plus pass/fail output summary for each edited `.allium`
- **Review / evidence ledger** — self-review findings, reviewer findings, and any corrections made; write `None` if none
- **Speculation avoided** — explicit note that no speculative helpers/contracts/imports were added beyond the task contract
- **Commit** — commit hash that landed the work


## Completion evidence

**Files changed**
- `specs/bedrock/bedrock.allium`
- `specs/projects/projects.allium`
- `tasks/92028-p1-in-progress--approved-task-placement-behavior.md`

**Settled facts encoded**
- `propose_task` interception in `specs/bedrock/bedrock.allium` now uses one blocking `AwaitingTaskApproval` path for every Git-backed parent conversation; the writing-mode fork split was removed.
- `ProposeTaskInvalid` now covers the same validation boundary for every Git-backed parent shape, so there is no parallel proposal-validity authority.
- `UserApprovesTaskCurrentConversation` / `TaskApprovalExecuted` preserve Continue here as same-conversation resumption only: no continuation edge, no fresh conversation, no provenance side effect, and no worktree transfer authority.
- `UserApprovesTaskNewConversation` / `TaskApprovalNewConversationExecuted` define Start in new conversation as a separate Open parent conversation with fresh worktree/workscope boundary, exact approved-task-only starting context, and derived-from provenance through normalized `ApprovedTaskSource` snapshotting.
- `ApprovedTaskSource` types taskmd vs plain markdown explicitly and keeps the original repo-relative markdown path plus exact body/title/priority snapshot independent of source worktree lifetime.
- `ApprovedTaskPlacementsAreMutuallyExclusive` and `ApprovedTaskDerivedChildCarriesOnlyProvenanceLink` provide the requested structural self-review proof: Continue here and Start new cannot both be authoritative, and the derived child carries provenance without becoming a continuation.
- Obsolete `ForkProposal*` review/spawn/promotion behavior and the old `UserApprovesTaskFreshWorkConversation` continuation-like handoff behavior were removed from Allium.

**Validation**
- Command: `allium check specs/bedrock/bedrock.allium specs/projects/projects.allium`
- Result: exit `1` with info/warning diagnostics only; no `severity: "error"` diagnostics were emitted after the edits.

**Review / evidence ledger**
- Self-review: confirmed the two placements are mutually exclusive structurally because `approved_continue_current` and `approved_start_fresh` now dispatch distinct approval rules, while `ApprovedTaskPlacementsAreMutuallyExclusive` forbids both authorities simultaneously.
- Self-review: confirmed there is no parallel proposal authority left in Allium by removing `ForkProposalIntercepted` / `ForkProposalInvalid` / `ForkProposalReview` and replacing them with the single `ProposeTaskIntercepted` + `ProposeTaskInvalid` boundary.
- Self-review: confirmed Start in new conversation has no continuation edge because `UserApprovesTaskNewConversation` keeps `continued_in_conv_id = absent`, and the projects child-placement rule records provenance via `spawned_from_conversation_id` / `ApprovedTaskDerivedFromRecorded` instead.

**Speculation avoided**
- No requirements, code, ADR, executive, or unrelated lifecycle/workscope-close surfaces were changed.
- No provisioning internals, continuation boundary invention, or extra review state machine beyond the requested blocking review / two placement outcomes was added.

**Commit**
- Pending
