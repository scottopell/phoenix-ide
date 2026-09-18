# ADR-055: Read-conversation cursors are versioned opaque strings

- **Status:** Accepted
- **Date:** 2026-09-17
- **Affects:** REQ-RET-008, REQ-RET-009, REQ-COMP-006
- **Supersedes:** The numeric read-cursor representation implicit in ADR-051

## Context

`read_conversation` pages rendered conversation content within a host byte ceiling. Continuing inside one oversized message requires a cursor to identify both the persisted message and an intra-message offset. Predecessor reads additionally require the cursor to remain bound to the ProductConversation, executing transcript, and requested predecessor selected by the host.

A single machine-sized integer cannot losslessly encode arbitrary host scope identifiers, target identity, persisted message identity, content freshness, and an unbounded intra-message offset. Encoding only sequence and offset permits a cursor issued for one transcript to be replayed against another transcript with a compatible position. Collision-bearing hashes or fixed bit partitions would narrow the valid message domain. Persisted numeric handles would require a new cursor store and lifecycle.

Persisted model history can contain tool calls with the numeric cursor shape. Reinterpreting those numbers under a new contract could return evidence from an unintended position or authority scope.

## Options considered

1. **Versioned opaque string payload.** Carry the full authority, target, position,
   message identity, and freshness values without server-side state.
2. **Authentication with a host secret.** Sign cursor payloads, but require a
   signing secret and lifecycle that are not available in every deployment.
3. **Numeric pairing or bit partitioning.** Preserve the old scalar type at the
   cost of collisions or limits on valid identifiers and offsets.
4. **Persisted numeric handles.** Resolve compact handles through a new cursor
   store with expiry, cleanup, and recovery semantics.
5. **Dual numeric/string support.** Keep accepting historical numeric cursors,
   despite their inability to identify the authoritative target and scope.

## Decision

`read_conversation` uses one shared versioned opaque string cursor contract for global and host-scoped reads. The cursor payload binds:

- cursor format version;
- host scope, including ProductConversation and executing transcript for predecessor reads;
- target conversation identity;
- persisted message identity and sequence;
- intra-message byte offset;
- a freshness digest of the rendered source message.

The host validates every binding before returning continued content. A target, scope, message, or freshness mismatch rejects the cursor and directs the model to restart without a cursor.

New persisted message identifiers are limited to 256 UTF-8 bytes at database admission. Rows that predate this constraint retain their actual identifiers: read output and percent-encoded citations may exceed the nominal page allowance by the narrow amount necessary to preserve resolvable legacy provenance. Phoenix does not rewrite legacy identifiers or substitute digest aliases for them.

Numeric cursors and unsupported cursor versions are rejected explicitly. Phoenix does not translate numeric cursors, persist cursor handles, or maintain a compatibility cache. Global Coordinator and predecessor-bound tools share the same cursor codec and read engine; their authority scopes remain distinct.

## Consequences

- Cross-target, cross-scope, and stale-source cursor replay fails closed.
- Full-content paging remains stateless and requires no schema migration or cursor storage.
- The internal tool schema intentionally changes from integer to string. Replayed historical numeric calls fail with restart guidance.
- Cursor payloads are caller-opaque transport values that agents must return unchanged. They are not cryptographically authenticated because Phoenix has no universally available host signing secret; host validation, rather than secrecy, rejects exact cross-scope, cross-target, and stale replay.
- Target-only continuation still decodes and renders the complete target message on every intra-message page. This decision does not remove repeated target materialization or the linear scope cost of exact search-index freshness verification.

## Rejected alternatives

### Authentication with an existing host secret

Rejected because Phoenix has no signing secret available in every deployment. Authentication sessions are optional and tied to deployment password configuration; using them would make cursor validity depend on whether browser authentication is enabled. Introducing and persisting a new signing key would create a new lifecycle and storage contract outside this decision.

### Numeric pairing or bit partitioning

Rejected because finite-width packing cannot represent the complete product of message identity, scope, freshness, and intra-message offset without collisions or domain limits.

### Persisted numeric cursor handles

Rejected because they require new storage, expiry, cleanup, and recovery semantics for an internal paging capability.

### Dual numeric/string support

Rejected because accepting the unsafe numeric representation preserves ambiguous replay behavior and creates a compatibility path with no authoritative target binding.

### FTS or SQLite row identifiers

Rejected because they are not the canonical message identity, may change under rebuild or database maintenance, do not encode source freshness, and still leave the offset-packing problem.
