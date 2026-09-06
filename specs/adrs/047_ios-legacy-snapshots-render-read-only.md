# ADR-047: Proven same-installation legacy iOS snapshots render read-only

- **Status:** Accepted
- **Date:** 2026-09-05
- **Affects:** REQ-IOS-002, REQ-IOS-005, compatibility requirements

## Context

Released iOS schema-v1 conversation snapshots contain conversation and message
content but no authority metadata. Rejecting every such snapshot discards the
core offline-reading value already delivered to users. Treating it as current
authority would instead permit stale or cross-account data to unlock sends.
The released installation retains server origin and a legacy credential in
Keychain, which can prove that the snapshot belongs to the current installation
scope without proving current transcript authority.

## Options considered

1. **Reject all schema-v1 snapshots** — fail-closed, but blanks previously
   available offline content after upgrade.
2. **Treat schema-v1 snapshots as authoritative** — preserves content and
   delivery, but fabricates authority absent from the stored record.
3. **Render proven same-installation snapshots read-only** — preserve content
   only when persisted server and legacy credential provenance establish the
   current installation scope; require a current init before delivery.

## Decision

Use option 3. Derive the legacy installation persistence scope from the
normalized persisted server origin and the legacy Keychain credential. A
matching schema-v1 snapshot may hydrate conversation and messages for rendering,
but it does not create `authoritativeSnapshotReceipt`, unlock outbox delivery,
or claim aggregate authority. Missing provenance makes the snapshot ineligible
cache. The first current authoritative init writes the normal exact-authority
snapshot format.

## Consequences

- **Positive:** released offline content remains visible after upgrade without
  weakening send or outbox authority.
- **Negative:** installations that no longer retain legacy credential provenance
  cannot use schema-v1 snapshots and receive the honest load error.
- **Neutral:** queued schema-v1 outboxes are not migrated or delivered.

## References

- ADR-034 compatibility guarantees
- `ConversationSession.init`
- `DiskConversationPersistenceStore.hasCachedSnapshot`
- `specs/compatibility/requirements.md`
- `specs/ios_client/requirements.md`
