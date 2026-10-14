# ADR-100: Web history selects original boundary messages

- **Status:** Accepted
- **Date:** 2026-10-12
- **Affects:** REQ-API-030, REQ-CONV-023, REQ-CONV-026, REQ-COMP-001

## Context

Continuous web history needs each completed context boundary to occupy the original continuation message's chronological position and exact identity. The ordinary snapshot API also serves Swift, whose existing adapter renders handoff-summary metadata separately from message arrays. Returning both the original continuation row and its handoff summary in that default response would duplicate the boundary in that consumer. Swift implementation is outside this web delivery scope.

Global history has its own aggregate kind and lifecycle. Reusing the ordinary snapshot's lifecycle and action fields would incorrectly represent Global as an ordinary conversation.

## Options considered

1. **Change the default ordinary response to original messages.** One projection is simple, but it changes the existing consumer contract and requires a paired Swift adapter change.
2. **Return both representations and have consumers hide one.** This makes a duplicated summary representable and relies on each consumer to select the same authority correctly.
3. **Select one explicit read representation.** Preserve the ordinary default metadata projection, let web request original messages, and expose a separate Global history view without ordinary lifecycle fields. This adds a bounded projection choice but avoids simultaneous duplicate summaries and a Swift implementation dependency.

## Decision

Choose the explicit projection. Ordinary requests select `boundary_representation=original_messages` for web history. The omitted/default `handoff_metadata` projection retains its existing summary metadata and excludes the represented continuation rows from message arrays. History segments structurally have no handoff-summary field, and their boundary roster contains only identity and provenance. Unknown projection names are rejected.

Both projections use the same aggregate read and cursor machinery. Global always returns original-message history using its own wire type, without ordinary lifecycle or Close actions. Newest transcript identity does not imply write readiness while a successor opening is reserved but unaccepted.

## Consequences

- **Positive:** Native web navigation points at the persisted continuation message; existing Swift API consumption does not need an implementation change; no response contains two representations of a handoff summary.
- **Negative:** Ordinary read projection tests must qualify both the default and explicit variants, including cursor paging. Two segment types and one shared ordinary metadata type are required.
- **Neutral:** This decision defines the supported read representations, not a generic cross-version, mixed-runtime, rollback, or database compatibility guarantee.

## References

- [API requirements](../api/requirements.md), REQ-API-030.
- [Conversation UI requirements](../conversation-ui/requirements.md), REQ-CONV-023 and REQ-CONV-026.
- [Compatibility requirements](../compatibility/requirements.md), REQ-COMP-001; ADR-034.
- `BoundaryRepresentation`, `OrdinarySnapshotResponse`, `aggregate_messages`, `ProductConversationHistorySnapshotView`, `CoordinatorConversationSnapshotView`.
