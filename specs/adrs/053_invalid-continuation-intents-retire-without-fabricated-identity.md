# ADR-053: Invalid continuation intents retire without fabricated identity

- **Status:** Accepted
- **Date:** 2026-09-17
- **Affects:** REQ-BED-021, REQ-COMP-001, REQ-COMP-002

## Context

A supported Phoenix continuation endpoint accepted an empty client message identifier while atomically creating a successor and pending opening-handoff intent. The persisted successor link is authoritative and usable, but the pending intent cannot satisfy the non-empty client-turn identity required for idempotent dispatch.

Forward migration must decide whether to preserve, transform, or retire this shipped shape. Inventing an identifier would falsely claim continuity with a client request and could create a duplicate opening message. Rejecting the row during every read would hide the already-created successor behind an internal error.

## Options considered

1. **Fabricate a replacement identifier** — retains automatic dispatch, but invents client identity and risks duplicate delivery.
2. **Keep the invalid intent and fail reads** — preserves bytes, but prevents the continuation endpoint from returning the durable successor identity.
3. **Retire only the invalid intent and preserve continuation topology** — removes an undispatchable obligation while retaining the authoritative parent-to-successor link.

## Decision

Choose option 3.

Forward migration deletes a pending continuation dispatch intent only when its client message identifier is empty. It does not alter the predecessor, successor, or their continuation link. Schema triggers reject future empty identifiers on insert and update, complementing the typed application boundary. A later continuation request therefore follows the existing-successor path and returns its identity without dispatching a fabricated turn.

## Consequences

- **Positive:** Supported historical databases upgrade without hiding an existing successor behind a deserialization failure.
- **Positive:** No client-turn key or accepted message is fabricated.
- **Positive:** Both the Rust type and SQLite schema reject future empty identifiers.
- **Negative:** The historical handoff payload on an invalid pending intent is retired because it cannot be delivered under its original idempotency identity.
- **Neutral:** Valid pending intents retain their original identifiers and retry behavior.

## References

- `specs/bedrock/requirements.md` — REQ-BED-021
- `specs/compatibility/requirements.md` — REQ-COMP-001 and REQ-COMP-002
- ADR-034: Compatibility guarantees are explicit and data-aware
