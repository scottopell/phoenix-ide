# ADR-254: Editable annotation cards own pending source highlights

- **Status:** Superseded by ADR-255
- **Date:** 2026-11-13
- **Affects:** REQ-PF-018, REQ-PF-019, REQ-PF-020

## Context

Inline reactions flatten their source quote and comment into the message textarea at Add to draft. Native selection also disappears when the user focuses the reaction input. The user wants passages to remain visibly marked throughout composition, editable comments after adding, and highlights to disappear after sending.

## Options considered

1. Keep formatted text and infer annotation identity from textarea edits. Arbitrary edits make removal and source-highlight tracking ambiguous.
2. Represent annotations behind a count badge with a separate editor. This saves space but hides comments and adds a step for small edits.
3. Keep individually editable annotation cards beside ordinary draft text. Each card owns its source and comment; removal and submission have explicit boundaries.
4. Interleave annotation blocks with ordinary text in one document editor. This provides flexible ordering but adds editing machinery beyond the first iteration's needs.

## Decision

Choose editable cards. `AnnotationDraftStore` belongs to the existing draft owner and is keyed by the writable composer's scope. A record owns a stable identity, immutable source passage, editable comment, and optional in-flight submission identity. Ordinary draft text remains independent; formatted annotation text is produced only for submission.

The transient reaction and pending cards feed native CSS range highlights without rewriting transcript DOM. Exact occurrence and prose-fragment anchors reconstruct ranges when virtualized content mounts. Cards reuse the existing paged source-return navigator.

An in-flight batch keeps its cards and highlights visible with editing disabled. Acceptance by the ordinary delivery path removes only that batch. Before delivery ownership is accepted, failed annotated submissions return to the composer instead of retaining a second failed queue copy. The existing offline queue may accept ownership while disconnected; this does not claim remote delivery. New annotations and ordinary text composed during a request remain untouched by its completion. Retired draft capabilities cannot modify a deleted or reused scope.

Cards and unfinished reactions remain session-local. This iteration does not add refresh recovery, server persistence, cross-device synchronization, or conversion of historical plain-text drafts into cards. The ordinary text draft retains its existing persistence behavior.

This refines ADR-056's append representation and preserves ADR-253's reaction-entry presentation and keyboard behavior. The existing batch reviewer remains independent.

## Consequences

- **Positive:** Visible source marks, editable comments, and composer removal follow the same annotation record.
- **Negative:** Session refresh does not recover cards; native highlight support is required for range painting, with unsupported capability logged. Cards cannot interleave with ordinary text in this iteration.
- **Neutral:** The backend and delivery queue still receive ordinary formatted text, and submitted highlights are not retained as permanent conversation history.

## References

- [Prose feedback requirements](../prose-feedback/requirements.md)
- [ADR-056](056_inline-reactions-append-without-taking-selection-focus.md)
- [ADR-253](253_reaction-pill-grows-with-wrapped-text.md)
- `AnnotationDraftStore`, `InlineMessageReaction`, `DraftAnnotationCards`, `ConnectedInputArea`
