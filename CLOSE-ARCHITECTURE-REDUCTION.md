# Close architecture reduction disposition

This is a scope ledger against shipped baseline `9885834f`, implemented in
reduction `d180a5e12` and now integrated with main `469647699`. The integrated
candidate remains **UNQUALIFIED**. Reduction Close run `45509934652` failed:
82 native Close DB tests, four shared-source core tests, frontend and specs
passed, but runtime test compilation failed on a missing `ContentBlock` import.
The next candidate includes that import repair and the reviewed hosted formatting
patch. Prior Close run `45491118337` passed and standard CI `45491139148` failed
at `cb7e80573`; neither qualifies this candidate. No local executable validation
was run. Preserved run `1068` has an unknown cause; do not attribute it to either
source or infrastructure without evidence.

Integrated `efee00293` Close run `45517506691` and CI `45517517914` also failed:
all 82 native Close DB tests and each browser custody test, including the two new
synthetic gate/cancellation/replacement cases, passed. Runtime compilation rejected
the attempted import's wrong module path, so public/Stop journeys did not execute;
the enclosing DB gate also lacked its runtime inventory. Frontend had one stale
tooltip assertion (114 passed), specs passed, and CI stopped on three Rust format
differences. The next candidate corrects the import to its verified declaration in
`domain::llm_types`, updates that assertion to explicit-retry guidance, and applies
the hosted format patch. It is not yet qualified; reduction does not revert to the
pre-audit successful candidate.

## Keep

- One shared executing-source predicate in core, used by DB and IDE callers. This
  is the domain contract, not a native task-approval feature dependency.
- Migration 172 identity/provenance state and replay behavior.
- Close's live custody and two-stage handback. Keep the existing physical deletion
  implementation and reuse it; do not rewrite deletion as part of this reduction.
- The live resource and `close_retirement` regression coverage that exercises
  those retained ownership and handback guarantees.
- Ordinary core compatibility coverage, including task-domain module tests. The
  Close workflow must not single out a task body hash test as its selector.

## Share

- `phoenix_core::domain::executing_source::verify_executing_source` is a read-only
  predicate, not a mutation capability. The DB `executing_source` adapter serves
  native Close initial admission, receipts, retry execution and source fencing;
  IDE `runtime/native_close_application.rs` uses the same predicate while retaining
  typed intent/correlation checks. Transaction and live-effect authority remain
  owned at their existing boundaries. Close CI selects this core module's tests
  directly, requiring a nonzero passing result and every named case separately.
- Existing Close physical cleanup is a shared implementation boundary: both
  retained native behavior and legacy callers keep using that implementation.

## Delete from this reduction

- Unexposed native task-approval DB/runtime modules and their test-only adapters;
  preserve the exact local work on `preserve/native-task-approval-cb7e80573`.
- Test-only alternate initial-adapter assertion coverage. Retain the live resource
  and `close_retirement` tests instead of preserving parallel adapter-only proofs.
- Narrow DB no-entry facades made unnecessary by the reduction, while retaining
  live recovery paths and all schemas/migrations required by the retained behavior.
- The Close workflow's standalone task-body hash test selector.

## Qualification boundary

Do not change Proposed ADR-284 history. Do not call the dirty reduction qualified,
claim local validation, or mark the native task-approval task done. The workflow
selector is a hosted-CI configuration change; this local documentation/edit pass
runs only `git diff --check`.

## Workflow test selection

The workflow selects `domain::executing_source::tests::` in `phoenix-core`.
It individually requires exact acceptance, non-executing/blank/wrong identity
refusal, tool-name/payload disagreement refusal, and missing/duplicate/remaining/
completed occurrence refusal. The separate `conversation_goals::prepared` tests
exercise their own predicate and are not evidence for this extracted helper.

## Preservation and compatibility residue

`preserve/native-task-approval-cb7e80573` points to full commit
`cb7e805735220fd260537a42e33e15baacaff70f`; it is a local preservation ref,
not a finished feature-only split PR. The native task runtime/DB modules and
fixtures remain recoverable there. Migration 172 and all later migration
identities/SQL remain intact; core reviewed-proposal serialization, state-machine
capture, checkpoint validation and provider replay remain supported. Those are
compatibility obligations, not a claim that the removed native tools are exposed.
ADR-284 stays Proposed and task 53021 remains unfinished.

The deleted `native_initial.rs` adapter was test-only kind-wide execution.
Exact/duplicate/missing/reordered observations remain covered in
`runtime/close_retirement.rs`; production resource admission and custody regressions
remain in `close_retirement/native_initial/resource.rs`. Initial metadata tests now
use `*_with_entry`, preserving wrong-snapshot, before-dispatch, stopped-source
and successful-persistence assertions. DB-internal compatibility facades are
restricted to tests; the IDE worktree fixture uses public
`try_settle_native_close_initial`. Historical recovery, retry wrappers with live
runtime callers, and schemas are retained. No generic test framework was added.
