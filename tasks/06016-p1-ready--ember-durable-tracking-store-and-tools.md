# Ember phase 3: durable tracking store and native tool

Parent: [06013](./06013-p1-ready--ember-consistency-and-durable-tracking.md). Depends on phase 1's contract. No scanner/model calls in this slice.

## Outcome

The ONE existing Ember singleton can retain, inspect, edit and deliberately retire things independently of transcript continuation, worker execution, PR state and generated summaries.

## Design boundary

Use a stable tracking-item domain type and application service. One item represents an independently meaningful concern/commitment, with no parent/child hierarchy initially. Prose captures purpose/current understanding and a resumption point or follow-through. Preserve the distinction between a user curiosity/bookmark and explicitly requested work; phase 1 chooses the minimal representation so bookmarks do not become work obligations. Structure identity, disposition, revision, timestamps, normalized ordered references and exact evidence. Use tracking/resolved/dismissed with nonblank retirement reason. Explicit reopening requires current evidence/revision and is not a scanner operation.

Bind list ownership to stable Coordinator product identity, not current transcript. References distinguish current conversation target (immutable instance + stable product), exact source (instance/transcript/message) and GitHub PR (repository/number). Source unavailability does not resolve an obligation; decide feature-scoped retention without cascading away records.

Persist references/evidence as child rows with foreign keys/ordinal and discriminated constraints. New timestamps are checked integer Unix microseconds. Do not place child collections in JSON or duplicate worker/PR state. Required retirement checks must explicitly reject NULL: SQLite CHECK of length(trim(NULL)) alone passes.

Use one compact bounded native surface for list/read/record/update/retire/reopen, provider-compatible schema and trusted invocation identity. Freeze minimal exact retry/conflict receipts in the spec before choosing operation-ID machinery. Reuse application authority and revision patterns without cloning every Goal prepare/apply detail.

Every manual mutation increments registry revision so scanner results can be fenced later. Retired rows/reasons/evidence remain available for matching and review. No deadlines, priorities, assignment graph, separate merge subsystem or runnable tracker states.

## Starting points

crates/phoenix-core/src/domain; crates/phoenix-db/src/{lib.rs,migrations}; coordinator_tools/authority.rs and goals.rs for admission/revision conventions; reminder_service.rs for trusted invocation receipts; runtime.rs::coordinator_tool_registry; compatibility requirements.

## Validation

Migration/schema tests; invalid/null retirement; typed reference constraints; two distinct commitments cited from one source message; repeated operation versus changed intent; concurrent edits/conflicts; local-authority commit classification; missing source/removed peer; data reopen. Test captured text as evidence, never user permission. Ordinary/child registries do not acquire mutation capability.

Recreate the parent plan's disposable SQL invariants in production tests; its ignored scratch prototype is not implementation. Done when committed records/retirement survive supported same-database restart and exact evidence stays intact, with precise bounded receipts and no dispatch/Goal/Authority side effects.
