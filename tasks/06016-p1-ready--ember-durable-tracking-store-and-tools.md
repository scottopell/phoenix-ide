# Ember phase 5: durable tracking store and native tool

Parent: [06013](./06013-p1-ready--ember-consistency-and-durable-tracking.md). Requires only the tracking-related terminology/baseline from [phase 1](./06014-p1-ready--ember-tool-contract-and-consistency-base.md). This task owns its record/native-tool/reconciliation contract before implementation; event and scanner design and the aggregate completion of phase 1 do not gate it. No scanner/model calls in this slice.

## Outcome

The ONE existing Ember singleton can retain, inspect, edit and deliberately retire things independently of transcript continuation, worker execution, PR state and generated summaries.

## Design boundary

Use a stable tracking-item domain type and application service. One item represents an independently meaningful concern/commitment, with no parent/child hierarchy initially. Prose captures purpose/current understanding and a resumption point or follow-through. Preserve the distinction between a user curiosity/bookmark and explicitly requested work; this feature's contract chooses the minimal representation so bookmarks do not become work obligations. Structure identity, disposition, revision, timestamps, normalized ordered references and exact evidence. Use tracking/resolved/dismissed with nonblank retirement reason. Explicit reopening requires current evidence/revision and is not a scanner operation.

Bind list ownership to stable Coordinator product identity, not current transcript. References distinguish current conversation target (immutable instance + stable product), exact source (instance/transcript/message) and GitHub PR (repository/number). Source unavailability does not resolve an obligation; decide feature-scoped retention without cascading away records.

Persist references/evidence as child rows with foreign keys/ordinal and discriminated constraints. New timestamps are checked integer Unix microseconds. Do not place child collections in JSON or duplicate worker/PR state. Required retirement checks must explicitly reject NULL: SQLite CHECK of length(trim(NULL)) alone passes.

Use one compact bounded native surface for list/read/record/update/retire/reopen, provider-compatible schema and trusted invocation identity. Freeze minimal exact retry/conflict receipts in the spec before choosing operation-ID machinery. Reuse application authority and revision patterns without cloning every Goal prepare/apply detail.

Every manual mutation increments registry revision so scanner results can be fenced later. Retired rows/reasons/evidence remain available for matching and review. No deadlines, priorities, assignment graph, separate merge subsystem or runnable tracker states.

## New evidence requires reconciliation

Appending evidence must atomically invalidate any claim that the saved resumption note has been reviewed against all available evidence. Use a conservative evidence-since-review rule, not a model restriction classifier: derive the latest evidence watermark from ordered evidence rows and retain the last explicitly reviewed watermark. Derive review-needed from that comparison rather than storing a second independently mutable flag. Scanner-created records start unreviewed.

Within this evidence contract, revised source observations append at a new position rather than silently mutating evidence behind the reviewed watermark. The marker must track actual evidence changes, not only row count or a scan timestamp. Define this alongside feature-scoped evidence retention before implementation.

Expose a typed review-required versus reviewed projection. Review-required records retain notes for inspection but cannot return the old resumption note as a ready next step. List/detail/context reads carry this state. It is distinct from tracking/resolved/dismissed disposition and grants no execution authority even when reviewed.

Only a trusted Ember reconciliation through the existing update service may advance the reviewed watermark, with expected item/evidence revision checks. It reads the new source evidence, retains any user pause/correction and revises the note as needed; concurrent evidence causes a conflict. Scanner append cannot clear this state, rewrite Ember prose, lift a pause or reopen a retired record. Exact duplicate evidence application must not manufacture new review work. No separate review workflow/tool is required.

## Starting points

crates/phoenix-core/src/domain; crates/phoenix-db/src/{lib.rs,migrations}; coordinator_tools/authority.rs and goals.rs for admission/revision conventions; reminder_service.rs for trusted invocation receipts; runtime.rs::coordinator_tool_registry; compatibility requirements.

## Validation

Migration/schema tests; invalid/null retirement; typed reference constraints; two distinct commitments cited from one source message; repeated operation versus changed intent; concurrent edits/conflicts; local-authority commit classification; missing source/removed peer; data reopen. Test captured text as evidence, never user permission. Ordinary/child registries do not acquire mutation capability.

Recreate the parent plan's disposable SQL invariants in production tests; its ignored scratch prototype is not implementation. Done when committed records/retirement survive supported same-database restart and exact evidence stays intact, with precise bounded receipts and no dispatch/Goal/Authority side effects.

Test an actionable note followed by appended pause evidence: its projection becomes review-required atomically, survives restart and cannot expose the stale step as reviewed. Test reconciliation racing another append, scanner attempts to clear review state, exact duplicate evidence and trusted creation/reconciliation with its actual evidence watermark.

Out of scope: fresh context/UI projection, scanner inference, new wake policy and completion-event implementation.
