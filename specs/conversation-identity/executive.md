# Conversation Identity — Executive Status

## Delivery boundary

This is staged v0.14 backend/spec work, not deployed behavior. Migration 129 is a new forward name-authority transformation; it does not expand or alter any qualified migration-128 production/incident registry, controller, recovery, or deployment admission. UI source integration is included. Compiled Rust qualification, genuine code generation, and rendered acceptance remain pending.

## Implementation

The root transcript `title` is the sole name authority. Migration 129 folds effective root names into it, drops persisted `chain_name`, releases all migrating root slugs before allocating in `(created_at, id)` order, and retains message and identity data. `conversation_name::allocate_slug_tx` allocates Unicode projections against shared transcript slugs and actual transcript/product identities. Ordinary rename, low-level root creation, legacy creation-shell publication, and product/fork insertion use the same projection rule in their write transaction. Creation claims and ordinary History/Close admission remain authoritative.

`canonical_route`, list projection, creation status, and ordinary Global reference output publish the encoded readable route. Product UUIDs remain API/cache identity. Exact transcript links remain explicitly pinned. Legacy chain-name endpoints write the same root title; the old response slot is null rather than another authority.

## Verification coverage

| Requirement | Authored coverage | Qualification |
|---|---|---|
| REQ-CID-001 | Legacy name round trips, prose generator tests, duplicate human-name rename/publication | Rust execution pending |
| REQ-CID-002 | Two-handle concurrent rename; deterministic numeric suffixes; actual ID collision; duplicate publication; low-level creation collision | Rust execution pending |
| REQ-CID-003 | Readable canonical list/snapshot/status routes and Unicode path-component encoding | Rust execution pending; UI acceptance pending |
| REQ-CID-004 | Stable ID aliases after rename, released-route reuse, existing exact-transcript tests | Rust execution pending |
| REQ-CID-005 | History/active Close fences, stale generated-name baseline, and stale creation-claim name/slug nonmutation | Rust execution pending |
| REQ-CID-006 | Effective-name migration, cross-root old-slug occupancy, column removal, stable message/continuation evidence; historical migration-005 checks | Rust execution pending |

Local TypeScript checking and focused identity/chain UI tests pass. Full UI and compiler-free fixture qualification are being reconciled; Rust compilation/tests and generated-type validation run on hosted CI, not the resource-constrained local host. Static review found and corrected legacy creation-status compilation defects, historical migration fixtures, and continuation rename response identity. These checks do not substitute for rendered acceptance.

The unchanged LLM Allium files (`anthropic`, `chat-completions`, `responses`) have parser failures on pre-existing comment syntax. This is recorded as an unrelated qualification blocker, not a naming-spec pass. The incident controller remains hard-pinned to 128; historical unit fixtures isolate that contract and separately assert rejection of the feature registry.

## Decisions

[ADR-128](../adrs/128_single-conversation-name-and-allocated-slug.md) records the single-name authority, allocated projection, and explicit absence of former-slug compatibility.
