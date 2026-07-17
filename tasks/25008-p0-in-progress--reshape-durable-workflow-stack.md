# Reshape and finish the durable-workflow stack

## Goal

Implement the accepted architecture in `docs/durable-workflow-grand-vision-review.md`: the smallest shared durability model that makes Phoenix's required incorrect states unrepresentable for one API-server process with bundled SQLite, while preserving the correctness already earned by the existing workflow work.

The governing test is: every mechanism must pay rent through a named invariant or concrete capability, and every removed mechanism must have an explicit replacement proof.

## Current stack reality

- PR #485 (normative durable-workflow architecture) and PR #486 (pure engine) are merged.
- PR #488 (`feat/workflow-persistence-engine-wake`) remains open at checkpoint `0409841aa5d583b3dfeb3752d008b395410625cb`.
- PR #489 (`feat/shadow-creation-durable-workflows`) remains stacked on #488 at checkpoint `90a98553858a2560ea55c0d4da12702ad78e6c21`.
- The accepted review is commit `7b92ea6e3` on `task-25007-grand-vision-review`.
- Preserve recoverable refs for all open heads before rewriting or force-pushing.

## Normative target

The revised requirements, Allium, and a new superseding ADR must encode:

- one scheduler authority per SQLite database;
- durable acknowledgment as the workflow-adoption boundary;
- direct turns accepted durably under stable client message identity;
- workflow generation, process incarnation, and effect-attempt stale-result fencing independent of leasing;
- leases only for reclaimable phases, with expiry never implying an external mutation stopped;
- structural execution/recovery classes: idempotency-keyed resend, externally observable/reconcilable, safely repeatable, and manual ambiguity only;
- long-running remote work as durable submit → handle receipt → reclaimable observation → terminal receipt;
- one canonical normalized delivery lifecycle and acceptance disposition;
- profile kind/version, total migrations, and explicit reconciliation/manual states instead of permanent protocol selector/shadow/drain machinery;
- `CoalesceLatest` as the explicit first scheduled-loop profile;
- no silent loss when active persisted work cannot migrate.

Run the pre-flight checklist in `specs/AUTHORING.md` before pushing specification changes. Requirements and Allium describe timeless current behavior; record the changed architecture in a new ADR rather than rewriting historical ADR rationale.

## Delivery sequence

### 1. Establish checkpoints and amend the normative base

- Record current PR metadata, heads, ancestry, live review-thread inventory, and remote backup refs.
- Update `specs/durable-workflows/requirements.md`, the durable-workflow/wake/creation Allium files, executive status, and ADR index with a superseding decision record.
- Remove steady-state requirements for universal leases and permanent selector/executor-registry/shadow/rollback/exact-drain machinery only after naming the replacement guarantees.
- Validate Allium and all cross-file references.

### 2. Reshape the merged pure engine

- Split universal attempt authority from optional reclaimable lease authority.
- Make recovery permission structural in effect/execution variants.
- Remove selector/shadow/drain states and transitions from the permanent engine core.
- Preserve deterministic reduction, typed IDs/effects, atomic transition planning, DAG/barrier capability, ambiguity, cancellation/compensation, durable deadlines, manual resolution, and generation/attempt fencing.
- Add model/schedule/property tests proving lease expiry cannot authorize policy-forbidden duplicate mutation and stale process/attempt results cannot commit.

### 3. Simplify unreleased persistence and repository APIs

- Rewrite unreleased workflow migrations rather than layering compatibility debt.
- Remove tables/APIs used only by permanent protocol deployment lifecycle or hypothetical competing DB claimers.
- Normalize canonical workflow, acceptance, effect, attempt, immutable evidence/receipt, delivery, profile intent/payload, and schedule facts with SQLite constraints as final authority.
- Represent optional leases through variant-specific structure rather than nullable tuple conventions.
- Collapse overlapping generic/wake inbox and obligation dispositions into one canonical delivery lifecycle.
- Make attempt/receipt origin, continuation ownership transfer, cancellation/terminal arbitration, and acceptance ordering transactional and structurally constrained.
- Verify FK checks, failpoints/crash boundaries, migration tests, and true multi-connection SQLite races.

### 4. Finish the wake vertical slice

- Port wake scheduling/execution/delivery onto the smaller repository model.
- Complete atomic registration/replay, exact continuation transfer, deterministic deadline/evidence precedence, cancellation, retry/reconciliation/manual ambiguity, and per-contract cancellation.
- Add pending/status REST and SSE projections and presentation parity for web, iOS, and `phoenix-client.py`.
- Prove the user journey end to end: a long build parks without consuming a turn, remains visible from at least two clients, survives restart/recovery boundaries, and wakes the conversation exactly once semantically.

### 5. Re-triage GitHub review threads

After the rewrite, classify every live non-outdated #488 thread as still applicable, removed by the revised model, superseded, or fixed. Respond and resolve with concrete commit/spec evidence; do not mechanically close comments. Re-check #489 separately.

### 6. Reassess creation adoption; defer expansion

- Rebase or rewrite #489 only after wake is complete and the migration strategy is proven.
- Replace permanent generic shadow machinery with migration-local comparison/cutover only if creation risk still justifies it.
- Do not expand into direct turns, scheduled loops, or remote executors in this task unless required to prove a core abstraction. Capture those as follow-on slices after wake ships.

## Required verification

- Normative spec validation, including `allium check` and `specs/AUTHORING.md` pre-flight.
- Focused pure-engine model/property tests.
- Repository migration, constraint, failpoint, transaction, and multi-connection race tests.
- Wake integration tests covering restart, stale attempts, uncertain submission, observer takeover, duplicate acceptance, cancellation/terminal races, continuation transfer, and exactly-once semantic delivery.
- Cross-client status/cancel/wake journey evidence.
- `./dev.py check` before final handoff.

## Completion criteria

- The checked-in normative contract matches the accepted oracle decisions.
- Permanent engine and schema state contain no selector/shadow/drain or universal-lease dimensions that serve only unsupported topology/upgrade promises.
- Stale completion rejection and ambiguity safety remain structural.
- Each semantic fact has one authoritative persisted representation; profile tables contain only non-overlapping domain data.
- The complete wake journey is observable and proven end to end.
- Surviving #488 review findings are fixed; obsolete findings are resolved with architectural evidence.
- Broader workflow adoption remains deferred behind the shipped wake vertical slice.
