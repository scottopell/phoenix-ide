Child of task 92018.

# Durable Close orchestration and cancellation contract across bedrock + durable-workflows

## Objective
Author the durable Close orchestration and cancellation behavior precisely across the bedrock and durable-workflow Allium/spec surfaces, without re-expanding into unrelated lifecycle areas.

## Exact target artifacts
- `specs/bedrock/bedrock.allium`
- `specs/durable-workflows/requirements.md`
- any existing `specs/durable-workflows/*.allium` files if they already exist and are the minimal place to encode this contract
- read-only context from `specs/bedrock/requirements.md`
- read-only context from `tasks/92017-p1-done--terminology-authority-requirements-basel.md`
- read-only context from `tasks/92018-p1-in-progress--lifecycle-close-allium-behavior.md`

## Settled facts from 92017 / 92018 that this task MUST encode
- Product lifecycle is Open/History only; Close is the action that transitions the conversation into History.
- Close is durable behavior, not a UI-only gesture, and must be specified where the durable orchestration/cancellation truth already lives.
- Context continuation remains one product conversation; Close semantics must not invent a separate lifecycle for continued rows.
- Latest-row authority remains derived from continuation topology; do not add duplicate ownership for “which row closes”.
- This task owns orchestration/cancellation only. It does not own WorkScope retirement classification, approved-task placement, or History/SSE projection.

## Required work contract
- Keep scope to durable Close orchestration/cancellation behavior across bedrock + durable-workflows only.
- Encode only settled contracts that already have implementation/requirement authority; if a needed helper/contract is not already grounded, narrow the behavior instead of inventing it.
- Do **not** add speculative workflow phases, placeholder events, guessed cancellation APIs, or cross-spec obligations owned by other child tasks.
- Preserve artifact voice: Allium for behavior, requirements only if needed to align existing durable-workflow normative text.
- Run `allium check` on every edited `.allium` file and record exact commands/results in the evidence ledger.

## Out of scope
- `specs/projects/**`
- `specs/work-lifecycle/**`
- `specs/pr-association/**`
- `specs/sse_wire/**`
- broad bedrock lifecycle topology beyond what Close orchestration needs
- code, ADRs, executive docs

## Evidence ledger required before marking done
Append a completion note with these headings:
- **Files changed** — exact paths changed
- **Settled facts encoded** — bullets mapping the task facts above to concrete requirements/Allium rules
- **Validation** — exact `allium check` commands plus pass/fail output summary for each edited `.allium`
- **Review / evidence ledger** — self-review findings, reviewer findings, and any corrections made; write `None` if none
- **Speculation avoided** — explicit note that no speculative helpers/contracts/imports were added beyond the task contract
- **Commit** — commit hash that landed the work


## Completion evidence

**Files changed**
- `specs/bedrock/bedrock.allium`
- `specs/durable-workflows/direct-chat-profile.allium`
- `specs/durable-workflows/wake-profile.allium`
- `specs/durable-workflows/requirements.md`
- `tasks/92026-p1-in-progress--durable-close-orchestration-cancellation-contract.md`

**Settled facts encoded**
- `specs/bedrock/bedrock.allium` now models a durable root-owned `CloseObligation` separate from `ProductConversation.lifecycle`, preserving the distinction between an active Close obligation and row/aggregate lifecycle state.
- Close admission is serialized to one active obligation per `ProductConversation`, only from the latest row, and it blocks while a row is in `awaiting_task_approval` or `awaiting_continuation`.
- Busy latest-row close now requires explicit `StopWorkAndContinueClosing` confirmation before settlement begins; non-busy close can advance directly into settlement.
- The pre-destruction window is cancellable through `UserCancelsClose(...)`; once `ResourceRetirementRequested(...)` is emitted the flow is committed and only completion/failure boundaries remain.
- Retirement completion now advances through the intentionally non-confusing lifecycle trigger `ProductConversationEnteredHistory(...)`, which fires only after `ResourceRetirementCompleted(...)`, not when the user merely asked to close.
- `specs/durable-workflows/direct-chat-profile.allium` adds the minimal direct-turn settlement contract: close-driven settlement targets the exact accepted-turn identity (`accepted_turn.workflow_id`, optional `canonical_delivery_id`) plus the current generation fence, and stale-generation settlement is explicitly a no-op.
- `specs/durable-workflows/wake-profile.allium` adds the minimal wake-owned contract: close settlement reuses existing `WakeCancellationRequested(...)` per pending binding instead of inventing a second wake-close state machine.
- `specs/durable-workflows/requirements.md` now normatively states the exact-identity/generation direct-turn settlement rule and the wake-profile reuse-of-existing-cancellation rule.

**Validation**
- `allium check specs/bedrock/bedrock.allium`
  - Result: exit `1` with pre-existing warnings only; no `severity: "error"` diagnostics. Representative warnings remain unused-definition warnings already present in the file (`UserQuestion`, `TaskProposal`, `RecoveryKind`, `TaskApprovalDecision`).
- `allium check specs/durable-workflows/direct-chat-profile.allium`
  - Result: exit `1` with info/warning diagnostics only; no `severity: "error"` diagnostics. New rules type-check; diagnostics are the file's existing unreachable-trigger / unresolved-path / unused-entity class warnings typical of isolated profile checks.
- `allium check specs/durable-workflows/wake-profile.allium`
  - Result: exit `1` with info/warning diagnostics only; no `severity: "error"` diagnostics. New rule type-checks; diagnostics are the file's existing unreachable-trigger / unresolved-path / unused-definition class warnings typical of isolated profile checks.

**Review / evidence ledger**
- Self-review: verified the new bedrock contract keeps Close durable and root-owned without turning `CloseObligation` into lifecycle state or adding a second cancellation state machine.
- Self-review: verified the retirement boundary is typed and stops at `ResourceRetirementRequested/Completed/Failed`, leaving loss categorization/fingerprint internals out of scope for task 92027.
- Self-review: verified direct-turn settlement language uses the exact identifiers already present in durable-workflows (`accepted_turn.workflow_id`, optional `canonical_delivery_id`, generation fence) rather than guessed IDs.
- Self-review: verified wake settlement reuses the existing `WakeCancellationRequested` authority name already present in `wake-profile.allium`.

**Speculation avoided**
- No loss-category taxonomy, retirement fingerprint internals, SSE projection behavior, PR/worktree implementation detail, or second close-specific cancellation workflow was added.
- The durable-workflows changes were kept to the owning files that already define direct-turn and wake cancellation/settlement semantics.

**Commit**
- Pending
