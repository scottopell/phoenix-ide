# Ember phase 3: bind completion events to their exact ending evidence

Parent: [06013](./06013-p1-in-progress--ember-consistency-and-durable-tracking.md). Requires only the event-specific baseline/contract subset described in [phase 1](./06014-p1-in-progress--ember-tool-contract-and-consistency-base.md), not completion of that entire task or any tracking/scanner design. This slice owns acceptance of its occurrence/retention/privacy/capture contract before implementation and is followed by [phase 4 projection/evaluation](./06021-p1-in-progress--enrich-ember-event-context-and-measure-f.md).

## User-approved outcome

The existing completion event identifies which input settled, who sent it, and the exact ending response. Delayed delivery must never attach a response from a later execution. User approved event enrichment after production showed repeated latest-message lookups. Lane B delivers this independently alongside the higher-priority manual memory lane; neither waits for scanner design.

## Reuse and scope

At deployed commit 4da5f81c, DirectTurnTerminalObligation already carries response_message_id. RuntimeExecutor selects it from the terminal PersistMessage effect. terminalize_authoritative_turn_in_tx invokes record_terminal_event_tx before deleting the obligation. That function already reads durable_turns.canonical_message_id. CapturedExecutionOccurrence likewise retains adopted steering input identity before settlement/deletion.

The wrapper terminalize_authoritative_turn_with_occurrence_at_cut calls that inner terminalization before settle_captured_execution_occurrence_tx. Capture one typed immutable ending-evidence binding at the wrapper boundary before the inner call deletes the obligation, and thread it to both appropriate event writers. Reading the obligation independently in each writer loses the later captured-steering path. AlreadyTerminal/TerminalReplay uses the original occurrence-bound evidence or explicit absence; never associate a newer adopted input with an older completion's response.

Carry this exact evidence through those boundaries. Include non-direct initial/creation/steering paths where an exact authoritative carrier exists; otherwise record explicit absence. Never query latest assistant message, infer a response from timestamps, or fetch new content during delivery.

## Contract

- Typed evidence distinguishes captured prose, tool-only/empty response, no exact response carrier, historical-not-captured and unavailable evidence. Preserve exact transcript/message locators and typed input-origin/sender facts where known.
- Label the association settled input, not sole cause: several queued inputs may have been co-consumed. Preserve existing occurrence keys and execution generations; event.source_generation is not transcript_generation.
- Freeze a bounded verbatim ending excerpt from stored typed content in the same transaction as the completion outbox fact. Its contract is an occurrence-time historical quotation, not a mutable current-message cache. Keep source revision/digest provenance sufficient to avoid misattribution.
- Initial evaluation capture cap is 1,024 UTF-8 bytes of response prose, clipped on Unicode boundaries. Persist capture-limit/version and omission provenance. Phase 4 may render less; increasing a later cap affects new captures only. Old truncated evidence stays truncated, never silently rehydrates from mutable source.
- Define the quotation's feature-scoped retention explicitly, including source edit/delete and recipient history; never silently rehydrate changed source content. The exact source may become unavailable without changing the recorded outcome.
- Persist a versioned digest of the full canonical response evidence, not just its captured prefix, alongside the exact source locator. Phase 4's event-bound follow-up reference must carry this expectation through every read/page. A bare message ID identifies a mutable row and cannot establish occurrence-time omitted content. No full historical-response copy is required.
- Normalize addressable evidence fields/ordered collections in columns/rows. Derive model/UI views from this one typed evidence authority. Do not create a second full transcript or mutable worker-status mirror.
- Preserve original diagnostic/provider evidence, accepted-input identities, deduplication, unwatch/Close suppression and continuation exclusions. No new per-send or queued-delivery event, summary model, wake scheduler or notification suppression.
- Existing events have no invented binding. Migration reports historical capture unavailable without destructive backfill or rewriting accepted inputs.
- V1 local capture/projection is the independently shippable target. Remote terminal-fact v1 has a closed deny_unknown_fields contract and no enrichment fields. Preserve that wire format; represent remote enrichment as unsupported/not captured. Any conversion that terminates the new capability must be typed and logged. Do not claim remote parity or add compatibility defaults.
- Tool-only completion has an exact pointer plus honest absence of prose. Extracting earlier outgoing-message arguments is deferred unless evaluation proves it necessary and exact execution attribution is available; do not implement a report-fragment subsystem by default.

## Starting files and contracts

crates/phoenix-db/src/workflow/direct_turn.rs::{DirectTurnTerminalObligation, persist_terminal_obligation_tx, terminalize_authoritative_turn_with_occurrence_at_cut, terminalize_authoritative_turn_in_tx, CapturedExecutionOccurrence, settle_captured_execution_occurrence_tx}; coordinator_watches.rs::{record_terminal_event_tx, record_steering_event_tx}; lib.rs::record_initial_execution_outcome_tx; crates/phoenix-ide/src/runtime/executor.rs terminal message persistence; phoenix-core domain InputOrigin/federation_subscription; DB migrations. Read specs/global-recall, conversation-events, conversation-retrieval, durable workflow, and compatibility before changes. Coordinate existing event-card owner/task 28007 rather than duplicate it.

## Done when

A completes; B emits a newer response before A's event delivery; A retains A's exact input/response/excerpt. Test steering/interaction races, tool-only and empty output, failures without prose, Unicode clipping, source edit/delete, historical absent capture, transaction failure/reopen/exact replay, and source ownership checks. No partial event/evidence commit or borrowed previous response. Local enriched versus remote legacy capability is explicit. Existing waits, mandatory cleanup failures and accepted input invariants stay covered.

A direct turn settles with an adopted steering/interaction occurrence, then retries after commit: both applicable event paths retain the correct source association and original ending evidence, without borrowing a later callback response. Test absent-carrier replay explicitly.

An oversized response is edited after capture: its unchanged frozen prefix cannot make the edited tail appear occurrence-bound. The captured full-content digest supplies phase 4's explicit match/changed/unavailable read result; an excerpt-only hash is insufficient.

Out of scope: changing notification frequency/timing, remote protocol expansion, model summaries, broad report-fragment extraction and tracker/scanner implementation.

## Current delivery evidence

PR [#1092](https://github.com/scottopell/phoenix-ide/pull/1092) implements this
component. Scoped capture, settlement, native read/schema/privacy, genuine codegen
and fixture web checks pass. Independent review accepted compact projection C's
[complete 39-trial comparison](../scripts/qa/ember-event-eval/results-cfda0b0.md);
the earlier capped invocation remains partial/unaccepted. This task stays in
progress until memory migration 164 is integrated, event migration 165 is rebased,
exact-head CI/review pass and the PR merges. No deployment or combined Ember
qualification is claimed.
