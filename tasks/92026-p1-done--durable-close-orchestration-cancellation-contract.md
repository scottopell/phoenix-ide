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
- `specs/durable-workflows/requirements.md`
- `tasks/92026-p1-in-progress--durable-close-orchestration-cancellation-contract.md`

**Settled facts encoded**
- `specs/bedrock/bedrock.allium` still models a durable root-owned `CloseObligation` separate from `ProductConversation.lifecycle`, but ordinary Close admission now rejects request-boundary attempts while the latest Open row is still awaiting task approval or continuation.
- Only one active `CloseObligation` may exist at a time, but a historical `completed` obligation no longer blocks repeated already-closed handling; Close remains latest-row-only and History rows do not create another obligation.
- Busy latest-row Close still requires explicit `StopWorkAndContinueClosing` confirmation before settlement begins, while cancellation before settlement starts ends immediately and cancellation during `settling_active_work` is queued onto the same durable settlement until ownership release finishes.
- Forced `CloseLossConfirmed(...)` from settlement has been replaced by a typed retirement-inspection boundary: post-settlement inspection either requests retirement immediately or binds a required confirmation to an inspection generation/fingerprint; changed inspection state invalidates stale confirmation and reinspects.
- `retirement_requested` and `needs_repair` are now retryable/non-terminal obligation phases. Retirement completion/failure is idempotent across retried attempts, retry from `needs_repair -> retirement_requested` is explicit, and retirement completion records obligation `completed` evidence before the lifecycle row becomes History.
- Once Close crosses into `retirement_requested`, user cancellation is no longer admitted. Close cancellation does not duplicate child/wake settlement; retirement simply stops progressing until the original latest-row settlement release boundary finishes.
- `specs/durable-workflows/requirements.md` now normatively states that lifecycle settlement targets only the exact latest-row accepted turn while still settling shared-`WorkScope`, sub-agent, and wake work via the existing typed child/wake boundaries of that same execution target.

**Validation**
- `allium check specs/bedrock/bedrock.allium`
  - Result: exit `1` with warnings/info only; no `severity: "error"` diagnostics after the reopened corrections. Representative warnings remain pre-existing surface/external-entity/unreachable-trigger warnings already present in the file.
- `allium check specs/durable-workflows/requirements.md`
  - Not run intentionally: this task only touched normative markdown in that file, and `allium check` is not applicable to `requirements.md` because it is not an Allium artifact.

**Review / evidence ledger**
- Reopened review addressed: approval/continuation gating moved to the `UserRequestsCloseConversation(...)` request boundary instead of post-creation waiting.
- Reopened review addressed: settlement-phase cancel now uses explicit `cancel_requested_during_settlement` and only completes once the same settlement release boundary finishes.
- Reopened review addressed: retirement inspection/confirmation now uses typed generation+fingerprint binding instead of forced `CloseLossConfirmed(...)` from settlement.
- Reopened review addressed: obligation `completed` evidence is explicit, and `needs_repair` can retry retirement without treating repair as terminal.

**Speculation avoided**
- No loss-category taxonomy, retirement category internals, SSE projection behavior, PR/worktree implementation detail, or second close-specific settlement/cancellation workflow was added.
- The reopened correction stays within the files previously touched by task 92026.

**Commit**
- `20eacb33fccf9305d15bfeebff841273841c5a7c` — `spec: correct reopened durable close review findings`
