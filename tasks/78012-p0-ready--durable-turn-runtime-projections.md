# Cut over runtime, UI, and SSE projections

Depends on all authoritative lifecycle cutovers.

Scope:
- runtime state/watch/SSE as one-way projections
- reconnect request authority
- RAII sequence reservations
- deletion of manual rewind discipline and remaining duplicate authority

Acceptance:
- Runtime/UI cannot independently mutate durable lifecycle truth.
- Uncommitted/no-write outcomes consume no SSE sequence structurally.
- Reconnect and sequence crash matrix pass.
- AGENTS authority/projection deletion checklist is complete.
- Full ./dev.py check passes.
