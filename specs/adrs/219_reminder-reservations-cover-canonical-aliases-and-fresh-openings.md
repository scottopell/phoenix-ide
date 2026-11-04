# ADR-219: Reminder reservations cover canonical aliases and fresh openings

- **Status:** Accepted
- **Date:** 2026-11-03
- **Affects:** REQ-API-033, REQ-REM-005

## Context

ADR-218 reserves reminder client identities at fresh input admission. Direct-turn materialization prefixes a client identity with its transcript ID, while steering drain retains the submitted identity. Message IDs are globally unique. An ordinary steering input can therefore occupy another reminder's canonical message ID even from a different conversation.

Manual continuation persists a successor and opening intent before dispatching its input. Rejecting a reserved identity only at Send Chat leaves that new successor blocked by its undispatchable intent. Existing durable opening intents remain forward obligations even when they own identities in the reserved namespace.

## Options considered

1. Reserve only raw reminder keys or aliases prefixed by the receiving transcript. This leaves canonical IDs occupiable from another conversation.
2. Replace canonical message allocation or introduce another reminder journal. This expands the change beyond reminder admission.
3. Use one lexical reservation predicate for raw keys and canonical aliases at fresh chat and continuation admission, retaining already-owned opening authority.

## Decision

Choose option 3. New arbitrary identities beginning with `reminder:` or containing `:reminder:` are reserved regardless of the receiving transcript. Fresh manual continuation checks the same predicate before successor or intent creation. Exact accepted replay still precedes the fresh reservation check.

Send Chat can discharge an existing continuation opening only after its existing intent, owner, source and text/policy validation establishes that authority. That validated owned intent is exempt from the fresh namespace restriction. A losing retry cannot replace its winning identity. This preserves an existing durable obligation without inventing another recovery protocol or namespace exemption supplied by a caller.

## Consequences

- **Positive:** Direct-turn canonical IDs cannot be occupied through ordinary steering aliases, including from other conversations.
- **Positive:** Fresh rejected continuation keys leave no successor or intent; existing owned openings can finish.
- **Negative:** Future arbitrary client IDs containing the canonical reminder delimiter are refused even when they name no existing reminder.

## References

- ADR-218: source-bearing ordinary reminder acceptance.
- `../api/requirements.md`, `../reminders/requirements.md`, `../durable-workflows/requirements.md`.
