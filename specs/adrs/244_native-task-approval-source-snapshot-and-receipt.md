# ADR-244: Bind native task approval to canonical source, snapshot and receipt

- **Status:** Proposed
- **Date:** 2026-11-08
- **Affects:** REQ-TAT-001–008; REQ-PROJ-003; REQ-BED-028/046; REQ-COMP-001/002; REQ-DWF-043

## Context

Native approval must prove the current Coordinator's canonical transcript/message/tool-use source, the exact reviewed proposal occurrence and whether a retry reports an already committed approval. Paths and identical bodies do not distinguish re-proposals. A receipt must share the approval transaction, not be an after-the-fact success cache.

The HTTP path uses mutation admission, `AwaitingTaskApproval`, `GitBackedActiveWorkScope`, task lifecycle admission, an unbound event and `RequestLlm`. It does not prove native source binding or impose blanket GoalStop/pause admission. Lifecycle rejects SubAgents and Close/history violations and admits only `Accepted` / `StandaloneAvailable`; no `TaskLifecycleAction` helper is claimed.

No independent SQL lookup or lifetime for reviewed body/path is required: review consumes the exact captured task-file Markdown, path, kind and digest as one aggregate. Canonical occurrence/full-message ownership and native receipt identity require SQL joins, foreign keys and deduplication.

## Options considered

1. **Trust supplied IDs/path and dispatch the unbound HTTP event.** Small, but cannot prove current source, exact proposal identity or replay semantics. Reject.
2. **Separate tool snapshot/receipt cache.** Duplicates pending review authority and risks receipt/approval transaction gaps. Reject.
3. **Fully relational reviewed-snapshot row with state reference and hydration.** Could give body/path independent SQL lookup/lifetime, but no such need is established. It would move the whole-read aggregate out of pending state and add state, HTTP/wire projection and hydration complexity without demonstrated value. Reject as unnecessary, not as structurally impossible.
4. **Single typed pending aggregate with existing canonical receipt ownership and scoped native approval receipt.** Preserve the existing aggregate/projection, add minimum typed canonical identity, and reuse the existing conversation/assistant canonical receipt for proposal ownership without a redundant occurrence table. Choose.

## Decision

- Keep one typed reviewed snapshot in `AwaitingTaskApproval`: the exact captured task-file Markdown, which is both the reviewed body and approved plan; repository-relative path, kind, digest of exact body bytes and canonical occurrence reference. There is no separate plan-text value. It is an earned whole-read aggregate, not field-addressed SQL data. Canonical checkpoint transaction validates the typed assistant/tool-use occurrence and captures the existing canonical receipt, assistant message and whole state atomically. No normalized snapshot row or hydration refactor; existing HTTP/wire projection is unchanged. Approved-task normalization consumes pending authority rather than duplicating it.
- Prove current Coordinator source against server-owned persisted `ToolExecuting` replay input/state through typed Rust before receipt lookup, matching full transcript/message/tool-use identity and canonical content. During live tool execution the source assistant message is not yet finalized in `messages`. Use the typed source-call occurrence for receipt identity, without requiring a finalized source-message foreign key. Do not persist assistant messages early, redesign checkpoints, or duplicate source content. Require a typed explicit target; inspection returns stable owner and concrete `CURRENT` transcript and approval freezes both without successor retargeting. `CURRENT` is not a free-floating argument.
- Read-only inspection returns the exact snapshot, including stopped pending proposals with ineligibility reasons and consequences; inspection does not resume or grant approval authority.
- Initial native approval requires explicit consequential intent and explicit `ContinueHere`. `StartFresh` returns typed unsupported refusal without substitution. HTTP fresh-start remains untouched. Require exact occurrence/snapshot and re-read the reviewed path/kind for body/digest comparison; identical-path/body re-proposal is still stale.
- Preserve lifecycle, Close/history, WorkScope and REQ-BED-046 publication/continuation boundaries. Persist an immutable native source-call-scoped receipt in the existing approval transaction alongside its domain facts and owed continuation. Retain target proposal/approval composite message-owner foreign keys and deduplication; reference the distinct proven source-call occurrence without a finalized source-message foreign key. The receipt references the proven canonical occurrence; it does not copy the snapshot.
- Current-source policy precedes receipt lookup; exact replay precedes pending-state and new-effect Stop admission. Return historical `Committed` without redispatch, even if the target has since stopped. Changed payload under the same scoped source key conflicts; replay creates no second continuation obligation.
- If a pending proposal lacks a canonical occurrence captured atomically with its reviewed snapshot, keep it inspectable, report native approval unavailable, preserve history and request actionable re-proposal. Do not infer, reconstruct, enrich or backfill identity from matching messages, equal plans, mutable disk or heuristic evidence; do not invent provenance or receipts.
- Fatal/unclassified and post-Git/pre-database failures fail-stop at their owning execution boundary, never `NotApplied`. If authority projection fails after commit, retain the committed durable fact and follow owning fail-stop; neither pretend successful publication nor classify the approval as not applied. `Committed`/requested is not dispatched, response received or provisioned. No generic exactly-once, downgrade, automatic Git rollback or new recovery framework is promised.

- Order new native approval admission against the explicit target conversation's Stop/pause latch at the owning pre-Git admission boundary. If Stop/pause is admitted first, return typed refusal with no new approval effects. Native approval never clears or bypasses the latch. A later Stop remains governed by existing dispatch and cancellation ownership; no global cancellation or rollback promise is made. Prove this ordering before enabling the native mutator. Suspension of a saved Goal alone is not Stop; HTTP approval policy is unchanged. Stop blocks new effects, not inspection or exact historical replay after current-source admission.

## Consequences

The change requires existing-approval-transaction integrity work for typed canonical occurrence ownership and immutable scoped receipts, while keeping the reviewed snapshot as one typed aggregate. Pending proposals without atomically captured canonical occurrence require legacy re-proposal for native approval; their history remains inspectable and preserved. Native placement is `ContinueHere` only; `StartFresh` is a typed unsupported refusal, and HTTP behavior is unchanged. No normalized snapshot row, hydration rewrite, or REQ-BED-028/REQ-PROJ-003 amendment is required absent a concrete conflict. The [executive](../task-approval-tools/executive.md) records status and the verification plan.

## References

- [REQ-TAT-001–008](../task-approval-tools/requirements.md).
- [Bedrock](../bedrock/requirements.md): REQ-PROJ-003, REQ-BED-028/046.
- [Compatibility](../compatibility/requirements.md): REQ-COMP-001/002.
- [Durable workflows](../durable-workflows/requirements.md): REQ-DWF-043.
