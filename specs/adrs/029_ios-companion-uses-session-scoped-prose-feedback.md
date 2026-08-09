# ADR-029: The iOS companion uses session-scoped prose feedback

- **Status:** Accepted
- **Date:** 2026-08-09
- **Affects:** REQ-IOS-019, REQ-IOS-020, REQ-IOS-021

## Context

ADR-028 expands the iOS companion into read-only project context and prose
review, but chooses durable anchored comments without defining their delivery,
retry, restart, or re-anchoring lifecycle. Phoenix already has a prose-feedback
contract in which notes live for one reader session and Send places structured
feedback into the editable conversation input.

ProductConversation can attach multiple `WorkScope`s while only one exact scope
is the live message execution target. Files may also remain visible as stale
read-only content. A note sent from another scope or an outdated file revision
could identify content the receiving agent cannot access or could point at a
different line than the user reviewed.

## Options considered

1. **Retain ADR-028's durable anchored comments** — preserves notes across reader
   and app lifecycles, but creates a second durable delivery state machine and
   requires a new server-side comment contract.
2. **Use session-scoped notes with guarded composer handoff** — reuses the
   prose-feedback contract and ordinary message delivery while allowing notes
   only for current content in the exact live target scope of a chat-capable
   conversation.
3. **Keep prose browsing entirely read-only** — avoids note state and routing
   concerns, but removes the feedback loop that makes mobile review valuable.

## Decision

Choose session-scoped notes with guarded composer handoff. The native reader
binds every note to the exact live-target `WorkScope`, file identity, and content
revision. Annotation and Send are unavailable when ordinary chat is unavailable,
the selected scope is not the live message target, or the content is stale.

Send follows the existing prose-feedback contract: it formats the notes into the
conversation's editable message input, clears the notes, and closes the reader.
Closing with notes requires explicit discard; notes do not persist after the
reader closes. This supersedes ADR-028's durable-comment choice while retaining
its bounded read-only project-context and prose-reader scope.

## Consequences

- **Positive:** iOS reuses one feedback and message-delivery contract instead of
  introducing another durable queue and failure lifecycle.
- **Negative:** Notes cannot be created for History/read-only conversations,
  non-target scopes, or stale content, and they do not survive closing the
  reader.
- **Neutral:** All attached scopes and stale files remain browsable; only the
  annotation affordance is restricted by message authority and content currency.

## References

- ADR-028, superseded by this decision.
- ADR-026, for ProductConversation lifecycle and WorkScope ownership boundaries.
- `specs/ios_client/requirements.md`
- `specs/prose-feedback/requirements.md`, especially REQ-PF-009 through REQ-PF-011.
- `specs/work-lifecycle/requirements.md`, especially REQ-WL-002 and REQ-WL-002b.
