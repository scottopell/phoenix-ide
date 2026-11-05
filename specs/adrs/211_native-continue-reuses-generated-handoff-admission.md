# ADR-211: Native Continue reuses generated handoff admission

- **Status:** Accepted
- **Date:** 2026-10-31
- **Affects:** REQ-GR-007, REQ-GR-019; REQ-BED-020, REQ-BED-021, REQ-BED-020b, REQ-BED-030

## Context

The user requests a native Continue tool after native creation and naming. ADR-191 intentionally grants only those earlier capabilities. Existing public Continue owns lifecycle checks, successor reservation, generated opening dispatch and capacity review. A Coordinator adapter must neither duplicate those rules nor promote a generated summary to new user instructions. A replay can already have a completed opening or a pending durable intent; treating both as a fresh request risks replacing the winning identity or requiring a provider for an already-established fact.

## Options considered

1. Keep Continue as a scoped-Bash/public-API recipe: retains public admission, but leaves exact source selection and generated-vs-literal opening policy to each invocation.
2. Expose the full public Continue request natively: supports literal/revised handoffs and user-agent controls, but unnecessarily grants new-instruction and recovery authority.
3. Provide a generated-only adapter over shared application admission, with a current-Coordinator reservation boundary and exact retained message identity: selected.

## Decision

Add a separate bounded exception to REQ-GR-007. `continue_conversation` accepts only an ordinary ProductConversation ID, predecessor transcript ID and persisted generated-summary message ID. Source identity comes from trusted tool context, and the server creates the proposed opening key. The reservation transaction revalidates the current Coordinator even on replay. Self-rotation remains excluded.

The shared application flow reserves at most one successor, preserves the winning durable opening identity and dispatches generated predecessor context with system provenance. New reservations review prospective capacity. Pending replay reviews the exact reserved successor and retained opening; completed replay requires no provider and performs no new dispatch. Failed capacity review does not erase a pending intent or synthesize a second successor.

Reservation and opening acceptance retain acknowledged durability classification. An owned application worker completes classification independently of requester lifetime. An unclassified boundary closes local runtime authority before releasing its owner. A closed application completion is distinct from success/refusal; the native adapter yields no ordinary result after closure, relying on the existing fatal-cancellation supervisor to drop the invocation. Fresh runtime outcome callbacks cannot borrow unrelated retained handoff-completion authority after closure. This does not revoke already-admitted effects or promise cross-process fencing or recovery.

This supplements, rather than rewrites, ADR-191. It does not add automatic-continuation settings, Goal admission, failed-work recovery, scheduling, structured-answer, review or approval authority.

## Consequences

- **Positive:** Native and public Continue share lifecycle and dispatch policy, and generated context cannot become caller-authored authority through native request fields.
- **Positive:** Completed replay reports established acceptance without provider availability; pending replay retains its exact winning intent on refusal.
- **Negative:** The API is deliberately narrower than public Continue. Literal/revised openings and explicit recovery require their own supported contract and authority.
- **Negative:** Classification, cancellation and result-publication boundaries require separate regression coverage; an application return alone is not final persistence proof.
- **Neutral:** No new persisted compatibility or startup-recovery guarantee is created.

## References

- [Global recall requirements](../global-recall/requirements.md), REQ-GR-007 and REQ-GR-019.
- [Bedrock requirements](../bedrock/requirements.md), generated handoff, exhaustion, capacity and inheritance contracts.
- [Compatibility requirements](../compatibility/requirements.md).
- [ADR-036](036_local-sqlite-authority-loss-fails-stop.md), [ADR-147](147_readable-generated-handoffs-retain-source-authority.md) and [ADR-191](191_native-create-and-rename-use-shared-application-actions.md).
- `ContinueConversation::run`, `ContinuationApplicationService::continue_from_coordinator`, `Database::continue_from_current_coordinator`, `dispatch_continuation_handoff`, and `ConversationRuntime::process_outcome`.
