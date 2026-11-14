# ADR-255: Compact annotation stack with modal editing

- **Status:** Accepted
- **Date:** 2026-11-13
- **Affects:** REQ-PF-019, REQ-PF-020

## Context

The user finds the inline editable cards from ADR-254 too tall. Pending annotations need recognizable positions and references without consuming the transcript viewport. Small comment tweaks must remain possible after Add to draft.

## Options considered

1. Keep the inline editable cards from ADR-254. Editing is direct, but the quotations and textareas consume substantial space even when idle.
2. Collapse annotations behind a count badge. This is compact but hides each annotation's identity and requires an extra selection step.
3. Expose overlapping card strips and move editing to a modal. The stack retains individual references while keeping the idle composer compact.

## Decision

Choose overlapping strips immediately above the ordinary composer, including for one annotation. Each strip exposes a source excerpt, comment preview, and a number allocated monotonically by its draft scope. Additions append in creation order; edits do not reorder, and deletion does not renumber surviving cards. Strips expose a 44px selection target; the stack has a bounded scroll region for larger collections. Cards overlap vertically without rotation or count-dependent horizontal offsets.

Selecting a strip opens the shared modal dialog with the complete quotation, editable comment, Delete annotation, Show in conversation, and Done. Comment edits write directly to the existing annotation record. Done, Escape, and backdrop dismissal retain edits and restore focus to the selected strip without scrolling. Deletion closes the modal and focuses an adjacent strip or the composer. Source navigation closes the modal only after finding the passage; unavailable or failed navigation keeps the comment and an explanation visible.

This supersedes ADR-254's inline editing presentation. Its annotation ownership, immutable source anchors, source highlighting, submission-batch isolation, retired-capability fencing, plain-text send representation, and session-local lifetime remain the decision here. In-flight annotations can be inspected but not edited or deleted; successful submission removes the corresponding modal together with its annotation.

## Consequences

- **Positive:** One, two, and several annotations share a compact, consistent shape. Each annotation remains individually recognizable and selectable.
- **Negative:** Comment edits require opening a modal. Previews truncate, and larger stacks require internal scrolling. Autosaved edits have no separate Cancel action.
- **Neutral:** This changes presentation without backend, persistence, or message-delivery changes. Light/dark desktop/mobile Ladle states and the screenshot gallery provide the design iteration surface.

## References

- [ADR-254](254_editable-annotation-cards-own-pending-source-highlights.md)
- [Prose feedback requirements](../prose-feedback/requirements.md)
- `AnnotationDraftStore`, `DraftAnnotationCards`, `SelectionDialog`
- [Annotation design gallery workflow](../../docs/qa/annotations.md)
