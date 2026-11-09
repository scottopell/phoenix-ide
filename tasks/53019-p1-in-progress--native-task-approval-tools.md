# Native task approval tools — draft review and owner decision

Draft packet: specs/task-approval-tools/requirements.md, executive.md, and Proposed ADR-244. Parent owns review. Implementation slice A is authorized for core/canonical capture foundations and focused tests only; feature enablement and runtime verification remain out of scope.

The reviewed body is the exact captured task-file Markdown and the approved plan; there is no separate plan-text value. Its digest covers the exact body bytes. The single typed `AwaitingTaskApproval` snapshot retains that body, repository-relative path, task kind, digest and canonical proposal occurrence reference. The canonical checkpoint transaction captures occurrence/full-message ownership and whole pending state atomically. A repeated identical path/body is a distinct proposal occurrence.

Native inspection requires current Coordinator provenance, canonical transcript/message/tool-use proof, and an explicit stable owner plus concrete `CURRENT` transcript returned/frozen without successor retargeting. Inspection is read-only and may expose stopped or otherwise ineligible pending proposals with consequences. If canonical proposal occurrence was not atomically captured with the reviewed snapshot, keep history inspectable, report native approval unavailable and request actionable re-proposal. Do not infer, reconstruct, enrich or backfill identity from a latest matching message, equal plan, mutable disk or other heuristic evidence. Preserve history and HTTP behavior.

Initial native approval requires explicit consequential intent and `ContinueHere`; `StartFresh` is a typed unsupported refusal without substitution, and HTTP fresh-start behavior is unchanged. Order admission against the explicit target conversation's Stop/pause latch at the owning pre-Git boundary: Stop admitted first returns typed refusal with no new approval effects. Never clear or bypass the latch. A later Stop remains under existing dispatch/cancellation ownership; no global cancellation or rollback promise. Prove this ordering before enabling the native mutator. Goal suspension alone is not Stop.

Keep one typed reviewed snapshot, not a normalized snapshot row or hydration rewrite. Normalize canonical occurrence/full-message ownership and immutable scoped native receipt relationships only. Receipt commits in the existing approval transaction, references the canonical occurrence (not a snapshot copy), and does not duplicate pending authority. Current-source policy precedes receipt lookup; exact replay precedes pending-state/new-effect Stop checks and returns historical `Committed` without Git or dispatch; changed payload conflicts. Receipt describes a historical commit, not current writable status or dispatch. Source provenance is proven through typed server-owned persisted `ToolExecuting` replay input/state, matching full transcript/message/tool-use identity and canonical content. The live source assistant message need not yet exist in `messages`; reference a minimum immutable source-call occurrence, not a finalized source-message FK. Retain target proposal/approval composite message-owner FKs. Do not persist assistant messages early, redesign checkpoints, or duplicate source content. Planned live-source regression must omit the finalized source message while retaining persisted execution state and reject forged/mismatched identity.

Preserve existing lifecycle, authority publication and continuation contracts. Fatal/unclassified and post-Git/pre-DB failures use owning fail-stop, not `NotApplied`. Post-commit authority-projection failure retains the durable approval fact and uses owning fail-stop, not `NotApplied`. `Committed`/requested is not dispatch, response or provisioning. No REQ-BED-028/REQ-PROJ-003 amendment, generic exactly-once/downgrade/recovery promise, API policy bypass or broad audit is part of this minimized shape.

Policy selection is resolved. Authorized scope includes slice A foundations and the bounded Part B leaf 1 DB read-only inspection/source-proof subset; full Part B actor/receipts/replay/Stop integration remains unfinished. Do not register/enable tools, change HTTP/Git/schema behavior, or touch `conversation_action_tools.rs`. Local Cargo/rustc/build/test/install/codegen remains prohibited. The owner permits the directly installed Rust 1.95 rustfmt only on new leaf files with `--edition 2021 --config skip_children=true`. Static `git diff --check`, commit and push to PR #1058 are authorized for hosted CI. Full feature integration and runtime verification remain NOT DONE.

## Part B leaf 1 — implemented, hosted validation required

- Added `phoenix-db::task_approval_inspection` and its module declaration only; no core, schema, mutator, tool registration, HTTP or Git behavior changes.
- `inspect_pending_task_from_coordinator` proves current Coordinator policy and exact actual executing transcript/message/tool occurrence before target lookup, in one SQLite read snapshot. It matches current tool name/raw input, the unique canonical tool block and the typed explicit-owner request; it rejects remaining/completed, cancelling, historical, duplicate, foreign and changed-payload sources. There is no finalized source-message prerequisite or fallback.
- Reuses the ordinary product aggregate read authority, resolving the owner's current latest transcript. The owned pending projection preserves exact task file/title/priority/body/provenance. `NoPending` is distinct; legacy absent provenance stays inspectable with typed `UnavailableRepropose`. Captured provenance explicitly requires later approval admission, not a permission grant. No disk read, reconstruction, backfill or writes.
- Eight focused tests written: real persisted ToolExecuting/no finalized source row; exact readonly aggregate; legacy versus no pending; target continuation; source mismatch matrix; current versus historical Coordinator continuation; request/identity/head gate failures; finalized-row non-rescue; validated-owner input. Read-only tests compare all persisted table contents.
- Validation: tests NOT RUN locally as instructed; direct installed new-file-only rustfmt and static diff checks only. Hosted compile/clippy/tests for this leaf are pending after push. Prior Part A head `05dcca22` passed hosted clippy; other prior lanes were pending at handoff.
- Explicit unfinished gates: full Part B actor and receipts/replay (including canonical-finalized same-call policy), owning runtime Stop inspection/admission and consequences, tool integration, native approval, dispatch/publication, Close and runtime verification. This leaf neither mutates nor adds disconnected Stop controls.

## Part B owning-actor readiness leaf — implemented, hosted validation required

The existing acknowledged runtime channel now has an explicit Event/admission-check
sum type. The internal manager check holds the shared mutation-admission lock through
the actor reply. The actor checks its own transcript and actual in-memory pending
aggregate against the frozen inspected owner/CURRENT/captured canonical occurrence,
plus ordinary lifecycle eligibility. A matching observation is named
`PendingMatchesRequiresFullAdmission`, never `Committed`, approved or dispatched;
it grants no authority or effect reservation. No native tools/controls are registered.

Actor-loop tests are authored for real UserCancel → persisted Idle/Task rejected →
typed stale check with cancellation writes measured separately; matching readiness
then later Stop; changed pending fields and same-body/new occurrences; legacy
unavailability; wrong actor and real DB CURRENT continuation. Existing Event
settlement/retirement and HTTP stale-event absorption are retained. Independent
ordinary-event queues have no chronological priority guarantee.

No Git, receipt/objective mutation, source-approval proof, HTTP policy, Close,
release or production change is included. Receipt/effect integration and full-B
revalidation remain NOT DONE. Local validation is scoped direct installed 1.95
rustfmt (edition 2021, skip children) and static diff review only. No local
Cargo/rustc/build/tests/install/codegen was run. Hosted exact-head checks are
required and pending; tests are authored, not claimed passed.

## Bounded Part B: private atomic DB receipt foundation

The DB-only sibling leaf adds normalized immutable source-call occurrence and
receipt tables in migration 156, after local 155. The receipt references the exact
source key, full target owner/transcript, canonical proposal/message witness and
approval context message; it stores no body/path/pending snapshot and has no FK to
the mutable continuation obligation. Live source assistant rows remain absent.

A private user/native settlement sum shares the existing authority transaction.
Native settlement proves current Coordinator plus exact live source typed input
and assistant block before receipt lookup, checks exact replay/conflict before
requiring target pending state, then inserts source/receipt in the same transaction
as all existing objective/scope/message/obligation/state writes. Native commit-error
classification additionally requires the exact receipt and snapshot/message facts.
No public native DB API, source-proof-bypassing receipt lookup, tool registration,
Git effect, actor integration or application-level Committed outcome is enabled.

Fourteen DB regression tests are authored for success with canonical target
checkpoints and absent source messages, all-write-stage trigger rollback, live
exact replay, changed identity, invalid/historical/cancelling/non-Coordinator
source policy, duplicate proposals, immutable/FK/foreign ownership constraints,
receipt-aware commit proof, queue independence and unchanged user approval.
**NOT RUN locally**: owner forbids local compiler/build/test/install/codegen.
Only scoped installed 1.95 rustfmt (edition 2021, skip children) and git diff checks
were run. Exact-head hosted CI is required; no green test claim is made here.

Remaining: finalized/canonical source replay policy (private helper currently
requires the exact live ToolExecuting call), actual pre-Git actor revalidation,
Stop/lifecycle/authority/effect ordering, Git admission, fail-stop and typed public
outcomes. Idle-state DB replay is not a runtime Stop-latch test. This is not full B.
The shared integration-owner seam is not released. No worktree, production,
factory, runtime/executor/API, registration, Close or build-hold changes.

Integration risk: Federation has an unpublished migration 156 reserved in another
checkout. Renumber one leaf and its ledger assertions before integrating both;
this leaf does not modify that checkout.
