# Native Task Approval Tools — Executive

## Status and authority

**Bounded Part A, Part B inspection/readiness and private DB receipt foundation implemented; NOT fully verified.** Part A captures typed provenance in the pending aggregate and preserves it on approval retry. Read-only inspection proves the actual executing Coordinator source. The DB foundation adds migration 156 and an internal native/user settlement sum sharing the existing approval transaction; it does not enable an actor, tool, Git operation or application-level `Committed` claim. No native approval or inspection tool is enabled or registered. Tests are authored but NOT RUN locally, by owner instruction. Only the installed Rust 1.95 formatter (edition 2021, skip children) and `git diff --check` are permitted locally; execution remains hosted-CI work on PR #1058. No generated type or wire change is delivered. The mutating actor, canonical completed-source replay policy, pre-Git revalidation and runtime Stop integration remain unfinished; native approval, Close and runtime verification are NOT DONE.

The native Stop/pause policy is resolved: the Coordinator, acting within saved standing authority / the commissioned Goal, selected refusal of new approval effects while the explicit target conversation latch is set, conveyed through owner `a52`'s question response. This is not new human approval or authority to resume. Existing human resume is required; a suspended saved Goal alone does not block approval. Inspection remains read-only and may expose stopped pending proposals with ineligibility and consequences. Exact historical receipt replay after current-source admission returns `Committed` without redispatch even after target Stop.

Owner's minimized single-aggregate direction governs this bounded implementation. The supported task remains in progress; the mutating actor and completed-source replay policy are separate follow-on gates.

## Private DB receipt foundation (not full Part B)

`native_task_approval` is private to the DB crate. Its fixed-field, versioned typed
encoding includes owner, concrete transcript, exact reviewed path/body/kind/digest
and proposal occurrence, title, priority, source occurrence, `ContinueHere` and
explicit approval/continuation intent. It is an internal command identity, not an
approved wire input or caller-supplied source proof. Full actual persisted tool
input and its assistant tool-use block must match that deterministic typed form.

The existing `persist_approved_task_authority_inner` transaction performs current
Coordinator and exact live `ToolExecuting` source proof before receipt lookup.
An exact receipt returns the DB durable fact without any new writes before
requiring target pending state. Changed identity conflicts. For a new settlement,
the target must still match CURRENT and the captured canonical pending proposal.
The normalized immutable source occurrence and immutable receipt are inserted in
that same `BEGIN IMMEDIATE` transaction as objective, WorkScope authority, approval
context message, continuation obligation and state. Commit-error classification
additionally proves the matching native receipt and snapshot/message artifacts.
The receipt references the immutable approval context and requested-continuation
fact, not the mutable obligation queue. It contains no copied body/path/snapshot.
A live source has no finalized message row and does not need a source-message FK.
Target owner/transcript, proposal message plus canonical receipt, and approval
message ownership are enforced by composite FKs; target proposal occurrences are
unique across source keys. UPDATEs of source occurrences and receipts are rejected.

Authored DB regressions cover canonical proposal checkpoint fixtures with and
without provider replay, live unfinalized source success, failure-trigger rollback
at each write stage, exact live replay after consumed pending/progress, changed
payload, invalid/historical/non-Coordinator/cancelling source refusal before lookup,
duplicate proposal conflicts, immutable/FK/foreign-owner constraints, receipt-aware
commit-error proof, dispatch/later-approval independence and unchanged user approval.
These tests are pending hosted execution, not claimed passed. The persisted Idle
replay test is not a runtime Stop-latch ordering test. Completed/canonical source
replay is deliberately refused until a precise same-call policy is integrated;
there is no public receipt-lookup API that bypasses source proof. A future actor
must still revalidate at the actual pre-Git boundary, own Stop/lifecycle/fail-stop
and effect ordering, and expose typed outcomes only after that integration review.

Migration **156** follows local 155 (`ordinary_self_goal_admission`). Federation
has reserved an unpublished 156 in a different checkout; integration must renumber
one leaf and update its ledger expectations before combining them. No other
checkout or the shared actor-integration seam is modified/released by this leaf.

## Bounded supplied facts; no new audit

- HTTP approval follows `mutation_admission` → reload `AwaitingTaskApproval` → `GitBackedActiveWorkScope` → task lifecycle admission → unbound event → `RequestLlm`.
- Lifecycle admission rejects SubAgents, Close/history violations, and states other than `Accepted` / `StandaloneAvailable`. `TaskLifecycleAction` does not exist.
- HTTP approval has no blanket GoalStop/pause predicate. Its unbound event does not prove a native source call.
- No independent SQL body/path lookup or lifetime need has been established. Preserve the single typed pending aggregate and existing HTTP/wire projection; reuse the existing canonical receipt for proposal ownership, and reserve immutable scoped native approval receipts for the later mutator. The native source call is proven by validating full transcript/message/tool-use identity against server-owned persisted `ToolExecuting` replay input/state using typed Rust; the assistant message is not yet a finalized messages row while its tool runs. The future source-call receipt must not require a source-message FK before tool completion.

## Minimized proposed impact

- Keep one reviewed-body/path/kind/digest plus canonical occurrence reference in the single typed `AwaitingTaskApproval` snapshot, an earned whole-read aggregate. The exact captured task-file Markdown is both the reviewed body and approved plan; there is no separate plan-text value, and the digest covers exact body bytes. Atomically checkpoint occurrence/ownership and whole state. No normalized snapshot row, duplicate receipt snapshot or hydration refactor. Existing approved-task normalization consumes pending authority rather than maintaining a parallel copy.
- Add only necessary typed core identity/provenance enrichment; preserve HTTP/wire projection. Pending state without a canonical proposal occurrence captured atomically with the reviewed snapshot remains inspectable but native approval unavailable; preserve history and provide actionable re-proposal. No heuristic reconstruction, enrichment or backfill.
- Initial native approval requires explicit consequential intent and `ContinueHere`. `StartFresh` is a typed unsupported refusal, never substituted; HTTP fresh-start remains untouched.
- Keep lifecycle and authority publication boundaries. At the owning pre-Git boundary, Stop admitted before native approval admission yields typed refusal and no new approval effects. Never clear/bypass Stop. A later Stop remains under existing dispatch/cancellation ownership; no global rollback promise. Prove this ordering before enabling the native mutator. Goal suspension alone is not Stop; HTTP policy is unchanged.
- Commit immutable source-call-scoped receipt in the existing approval transaction, referencing canonical occurrence with full owner/message relationships. Current-source policy precedes lookup; exact replay precedes pending/new-effect admission and never redispatches. Changed payload conflicts.
- Fatal/unclassified and post-Git/pre-DB failures use owning fail-stop, not `NotApplied`. Projection failure after commit retains the durable fact and uses owning fail-stop, not a false no-effect result.

No REQ-BED-028/REQ-PROJ-003 amendment is required by this minimized aggregate model absent a concrete conflict; none is identified in the supplied facts. REQ-BED-046, REQ-COMP-001/002 and REQ-DWF-043 retain authority. The prior assertion that bedrock amendment and projection refactoring were prerequisites is withdrawn.

## Owning-actor admission-readiness leaf

The internal `check_native_task_approval_admission` request travels through the
same acknowledged runtime channel as acknowledged events. It freezes the inspected
stable owner, concrete transcript and exact captured pending snapshot, then checks
the actor's own in-memory pending state, proposal occurrence, CURRENT membership
and ordinary lifecycle eligibility. Its matching result is explicitly
`PendingMatchesRequiresFullAdmission`, not approval, dispatch, a reservation or an
externally released authority token. It does not prove Coordinator source authority
or run Git, write an objective/receipt, or install native approval controls.

The manager holds the shared target mutation-admission lock through the reply;
the actor does not reacquire it. This serializes the check with HTTP mutation
admission, not chronological arrival across the independent ordinary-event and
acknowledged-request queues. A real Stop processed first consumes pending approval
into Idle and makes the frozen check return typed `StalePending`. Stop processed
after a matching readiness observation retains ordinary cancellation semantics;
there is no completed approval to roll back. The existing acknowledged Event
settlement/retirement branch and absorbed stale HTTP-event behavior are preserved.

Authored actor-loop regressions cover real Stop before a frozen check, matching
readiness followed by real Stop, all pending fields and same-body new occurrences,
legacy unavailability, wrong actor identity and CURRENT advancement using the real
DB continuation API. They measure cancellation writes separately and check for no
approval authority, tool/LLM execution or objective additions. These regressions
are **not locally executed**; hosted exact-head compile/test results are required.
This leaf does not complete any end-to-end native approval requirement. Receipt and
actual effect integration must revalidate full source/target authority at the
owning effect boundary; they remain NOT DONE.

Hosted run `42569939431` for actor leaf `12603354b` found a `TaskSource::detect`
Option/Result pattern mismatch and the predecessor DB test's implicit
`CancelCause` default lint. Both are corrected in the follow-up; its exact-head
hosted validation remains pending. No actor regression is claimed passed.
Hosted run `42570366055` then type-checked the corrected leaf but rejected the
request enum's size imbalance under clippy; the Event and frozen-snapshot payloads
are now boxed on the same channel. Other build/musl lanes failed during sccache
setup with HTTP 504, before running tests. New exact-head qualification is required.

## Coverage and proposed verification

Part A (canonical pending-state capture) and Part B leaf 1 (executing-source proof/read-only inspection) have focused tests written but NOT RUN locally. Leaf 1's `Database::inspect_pending_task_from_coordinator` performs current-Coordinator admission before target lookup in one SQLite read transaction, matches the source transcript/message/current call and unique canonical block name/input against the typed request, then reads the ordinary product aggregate's CURRENT member. It returns owned exact pending fields and provenance, distinct `NoPending`, or legacy `UnavailableRepropose`; it neither reads task files nor reconstructs provenance. A nonempty occurrence alone is not classified as captured: within the same read transaction it requires the existing canonical receipt keyed by target transcript and proposing assistant, plus that actual assistant message owned by the target. The reused canonical-write validator checks unique tool-use identity across tool names, typed `propose_task` path, reviewed body digest and task kind. Missing messages/receipts, foreign ownership and mismatches preserve the exact snapshot and return typed `UnavailableIntegrity` with a repair/re-propose consequence. Twelve focused tests are authored, including actual ordinary/provider canonical target checkpoints with the Coordinator source assistant still absent from messages, missing/foreign receipt/message, forged occurrence, duplicate/wrong-name calls, stale input path, body/digest/kind drift, existing source-admission/continuation cases and unchanged persisted tables. Scoped direct rustfmt 1.95 (edition 2021, skip children) and static diff checks are the local validation; hosted compile/clippy/tests remain required. The leaf does not inspect the runtime Stop latch and its captured-proposal result explicitly requires later admission; no approval eligibility or full REQ-TAT-002 stopped-state explanation is claimed. The private DB receipt foundation is described above; remaining Part B mutating actor/completed-source replay/Stop work, Part C (native approval), and Close are deferred.

| Requirement | Implementation | Tests | Runtime | Focused verification to add |
|---|---|---|---|---|
| REQ-TAT-001 | PARTIAL B leaf 1: executing-source identity/current Coordinator admission and explicit owner/CURRENT inspection in one read snapshot; no tool/actor admission | WRITTEN, NOT RUN | NOT VERIFIED | Current Coordinator; full transcript/message/tool-use identity matched against persisted typed `ToolExecuting` replay input/state; forged/mismatched state rejected; source assistant messages row absent while tool runs; explicit owner and concrete CURRENT returned/frozen; no successor retargeting |
| REQ-TAT-002 | PARTIAL B leaf 1: exact read-only pending aggregate with same-transaction target receipt/message witness and typed integrity unavailability; no runtime Stop explanation or tool surface | WRITTEN, NOT RUN | NOT VERIFIED | Exact aggregate inspection; stopped pending and missing-occurrence proposals remain inspectable with ineligibility/consequences; missing occurrence reports native approval unavailable; no mutation |
| REQ-TAT-003 | NOT DONE | NOT DONE | NOT DONE | Explicit ContinueHere; typed StartFresh refusal/no substitution; HTTP unchanged; stale identical-body re-proposal; disk drift; lifecycle/Close/history; prove pre-Git Stop-first ordering returns typed refusal with no new effects before mutator enablement; later Stop follows existing cancellation ownership; Goal suspension alone is not Stop |
| REQ-TAT-004 | PARTIAL A: typed occurrence/digest/kind captured before canonical checkpoint, validated against same-round assistant/tool-use and receipt; no separate occurrence table | WRITTEN, NOT RUN | NOT VERIFIED | Atomic canonical receipt/whole-state checkpoint and rollback; foreign occurrence rejection; same-body/new-assistant identity; unchanged HTTP/wire projection; approved normalization consumes pending authority |
| REQ-TAT-005 | PARTIAL B DB foundation: private same-TX source/receipt settlement and live exact-source replay; no completed-source policy or effect actor | WRITTEN, NOT RUN | NOT VERIFIED | Receipt/approval atomicity; scoped target-proposal full-message FKs and minimum source-call occurrence reference (no finalized source-message FK); source-call identity validated against typed persisted replay input/state; source gate before lookup before pending/Stop; exact historical replay even stopped, no redispatch; payload conflict |
| REQ-TAT-006 | NOT DONE | NOT DONE | NOT DONE | Committed/requested vs dispatched/response/provisioned; fatal/unclassified and post-Git/pre-DB fail-stop; post-commit projection failure retains committed fact; no NotApplied or second continuation obligation |
| REQ-TAT-007 | PARTIAL B leaf 1: typed legacy unavailable/re-propose and invalid-witness integrity results, no inference/backfill/disk read; no tool surface | WRITTEN, NOT RUN | NOT VERIFIED | Missing atomically captured canonical occurrence keeps proposal inspectable but native approval unavailable; preserve history/actionable re-proposal; no heuristic reconstruction, enrichment, backfill, fabricated provenance/receipts or hydration refactor |
| REQ-TAT-008 | NOT DONE | NOT DONE | NOT DONE | Authority publication before work; owning projection fail-stop; no Goal start/stop or pause clear |

## Review gates and exclusions

1. Owner authorized bounded Part A; proposed ADR-244 reflects the existing canonical receipt as its ownership witness.
2. Part A head `05dcca22` passed hosted clippy. Leaf head `5335c1ee` subsequently failed hosted compilation in its dynamic SQL test helper, while musl/UI setup also encountered hosted sccache HTTP 504 failures. The bounded witness fix corrects the two introduced compiler errors; exact-head hosted qualification remains pending. No local compiler, build, tests or codegen was run. Complete source actor/receipt/replay and owning Stop admission before enabling any native mutator. The private receipt foundation does not satisfy the remaining end-to-end native approval gates.

No native execution, local build/tests/codegen, registration, RC2, release or production work is included in Part A or Part B leaf 1. A draft PR triggers standard hosted CI; runtime behavior remains unverified pending those results. The README index labels ADR-244 Proposed; Stop/pause policy is resolved but its implementation is deferred.
