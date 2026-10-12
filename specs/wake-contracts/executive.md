# Wake Retirement — Executive Summary

## Decision and Scope

The user explicitly approved removing the durable wake feature. ADR-098 records
that decision; ADR-006, ADR-011, and ADR-012 remain historical context, not current
execution authority. The wake registration/router/profile contract and its Allium
models are retired, without a replacement framework or promise of redesign.

## Current Reality

The candidate branch removes operational wake admission, worker startup,
automatic adoption, and wake-only lifecycle coupling. Migration 126 records
explicit feature retirement additively for workflows, occurrence pointers, and
coordinator wake events. Predecessor local migration and runtime regressions have
passed; the historical prompt, owned-deletion, and coordinator-event corrections
have source-only authored regressions awaiting execution. Integration and deployment
are separate and are not claimed here.

Historical wake workflows, effect payloads and statuses, bindings, receipts,
messages, deliveries, acceptance records, and links must remain intact. Additive
feature-retirement metadata makes those workflows inert without falsifying their
historical execution outcomes. Historical wake pointers remain evidence only.

Already-admitted DirectTurns, including ones originating in wake delivery, remain
normal direct-turn work with their existing ownership, recovery, cancellation,
and settlement authority. Unaccepted wake delivery must not admit new work.
Ordinary Bash waits, tmux operations, sub-agent fan-in, and Close are not replaced.

## Requirement Status

| Requirement | Status | Verification evidence / boundary |
| --- | --- | --- |
| REQ-WAKE-RETIRE-001 no wake execution authority | Candidate corrections awaiting validation | Registration, worker, adoption, and wake lifecycle gates removed; predecessor `startup_does_not_resume_historical_wake_pointer` passed. Coordinator pending discovery now excludes explicitly retired events, and database admission rejects their new direct-turn/steering delivery. |
| REQ-WAKE-RETIRE-002 additive retirement migration | Candidate corrections awaiting validation | Predecessor migration receipts cover byte-preserving upgrade/reopen, workflow statuses, delivery shape, and rollback. The synthetic fixture now includes pending, accepted, and suppressed coordinator wake events plus all other subscription source kinds; exact-event retirement, unchanged-history, discovery, and admission assertions are authored but unrun. |
| REQ-WAKE-RETIRE-003 already-admitted DirectTurns | Implemented in candidate branch; locally verified | `historical_wake_pointer_does_not_fence_or_settle_direct_turn` passes; Direct Turn database and runtime ownership/recovery regressions pass. |
| REQ-WAKE-RETIRE-004 history without replacement feature | Implemented in candidate branch; locally verified | Historical storage and pointers retained. Runtime selection passes 802 tests with one existing ignored test; tool suite passes 533 tests with one macOS registry-test failure separately tracked. |
| REQ-WAKE-RETIRE-005 forward-only compatibility | Specified | Apply the existing compatibility policy; no downgrade or wake-restoration guarantee. |

## Verification Boundary

The source-only corrections restore `render_messages`' historical adoption filter
without a wake runtime type and restore binding-selected workflow deletion in
`delete_conversation_row_with_dependents` before owner deletion. Authored regressions
are `ordinary_new_turn_prompt_excludes_unadopted_historical_wake_results`,
`permanent_delete_removes_only_owned_retired_wake_workflows_and_children`,
`wake_feature_retirement_excludes_coordinator_wake_events_without_rewriting_history`,
and `wake_feature_retirement_fences_event_admission_but_keeps_accepted_direct_turns`.
The deletion fixture checks foreign keys before and after deletion and retains a
separately-owned wake workflow and unrelated non-wake workflow. These corrections
have not been compiled, formatted, or tested; predecessor receipts below do not
qualify them.

Local qualification is not deployment evidence. The initial complete database
run exposed nine legacy-fixture failures because partial-schema fixtures admitted
the new migration; those fixture boundaries were corrected and all 92 migration
tests subsequently passed. Existing Direct Turn, Close, creation, and shared
foundation tests passed in that database run. Full project-check completion is
not claimed by these focused receipts.

The Python client suite passes 14 tests, including absent wake commands/methods.
TypeScript, UI lint, and Vitest qualification passed after the surface removal.
Workspace all-target Clippy finishes with exit 0 and warnings denied. Its guard
recorded exit-time descendant cleanup; final process/group absence was verified
before reservation release. Changed Allium models, task metadata, and Rust
formatting validate. The shared
ADR chain is reconciled against actual federation merge
`1839a37f67c27d13f479068717d59a5aa413c450` and resource merge
`eec3fae65f113705733cbd8c5f98b97a03fdcf81`. The candidate preserves resource
ADRs 095–096 and ledger 124–125, then adds wake ADR-098 and migration 126. P1 retrieval merge
`be1fbc52c394e299e48268c4a5079dc22851f5ac` is the actual cumulative base;
its bounded-retrieval ADR-097 and implementation are preserved.
ADR/spec shape validation passes on that cumulative source. Earlier qualification
receipts retain their exact predecessor heads; final cumulative test and review
receipts are reported on the pull request rather than silently relabelled.
No deployment is implied by local qualification.
Old wake feature tests are historical evidence, not acceptance of retirement.

## Migration-aware rollback boundary

Retirement is a forward migration, not a reversible switch. Replacing only the
binary neither undoes migration 126 nor establishes authority to resume retired
wake obligations. Older code does not interpret the new retirement metadata;
using it against the migrated database is unsupported and may reinterpret
preserved historical status as active work. A binary-only rollback therefore
cannot promise continued retirement or safe wake restoration.

A supported version rollback follows the existing compatibility policy: offline
restoration of a matching pre-upgrade database and binary, or a separately
specified qualified paired deployment transaction. Such a restore returns the
older database state rather than reversing retirement in place, and may contain
its older obligations. No automatic reexecution, down migration, data purge, or
new rollback subsystem is introduced by wake removal.

## Authorities

- `requirements.md`: retirement and historical-data contract.
- `../durable-workflows/requirements.md`: retained foundation, direct-turn, and creation behavior.
- `../compatibility/requirements.md`: forward migration and paired rollback policy.
- `../adrs/098_retire-wake-execution-and-preserve-history.md`: explicit removal decision.
