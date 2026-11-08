# ADR-243: Federated Native Create and Rename Use Destination Actions

**Status:** Accepted

**Date:** 2026-11-08
**Successor to:** [ADR-191](191_native-create-and-rename-use-shared-application-actions.md)

## Context

ADR-191 established native Coordinator create and rename as adapters over shared application actions. Federation adds an optional destination selector to those capabilities. A remote implementation must preserve the same creation and naming semantics without treating a peer's transcripts as destination-local authority, duplicating mutation policy, or falling back to local execution after a selected peer fails.

Local creation already uses a caller-chosen request UUID as durable retry identity and carries a closed creation intent containing a destination-scoped directory, objective, model, effort, language, and submit-objective start intent. Local rename already uses a durable operation UUID, product/root/revision fences, and an atomic historical receipt. Replacing these identities or reducing remote creation to objective text would create a second semantic model rather than federate the native actions.

Remote attribution crosses an instance boundary. The source can attest the exact Coordinator tool call from trusted execution context; the destination can authenticate the source instance from peer credentials. The destination cannot prove a foreign transcript by resolving it as one of its own rows. Durable retries therefore need peer-qualified source provenance, not fabricated destination-local ownership.

## Decision

The native create and rename request variants accept an optional destination selector. Omission and the exact `local` selector invoke the local shared application action. A non-local selector resolves one exact saved peer. Self-selection, unknown or ambiguous selection, peer authentication or transport failure, unsupported endpoints, and invalid responses fail without local or alternate-peer fallback.

Remote option discovery runs against the selected destination. Remote create preserves the local native creation intent and uses its request UUID as the durable operation identity. The destination revalidates the directory and model-related choices in its own namespace, applies its own defaults for omitted optional selections, and invokes the same durable creation acceptance action used by local native and HTTP ingress. Success is explicitly `accepted` and reports durable acceptance, not publication or execution. Uncertain creation is reconciled by reading that exact destination by request UUID for `unknown` or one complete provisioning, publication, route, and initial-execution snapshot; a known snapshot is visible only to the authenticated peer recorded as acceptance owner, and reconciliation never falls back to local or another peer.

Remote rename carries the existing local rename intent: operation UUID, stable ordinary ProductConversation ID, expected canonical root transcript ID, nonnegative expected name revision, and requested title. The destination validates the ordinary target kind, invokes the shared naming action, and preserves its closed result classification and atomic historical receipt semantics.

Before remote dispatch, the source revalidates current-Coordinator authority and derives the source ProductConversation, transcript, assistant message, and tool-use locator from trusted execution context. The destination derives source instance identity from authenticated peer credentials, authorizes that peer for the operation, and persists a peer-qualified source locator with creation acceptance or rename receipt. Foreign source identifiers are provenance, not destination-local ownership claims.

Peer qualification is stored atomically with the complete source-call locator. Internal-native creation jobs and rename receipts require every peer-qualified locator field; non-native and unknown-historical creation origins carry none. Foreign identities have no destination-local foreign keys. The rollout preserves every existing locator value and backfills the database's durable local instance identity only onto internal-native creation jobs and retained rename receipts that already have complete native locators; it does not manufacture provenance for provenance-absent creation rows.

Create-option discovery, rename-option discovery, create, create-status, and rename use distinct strict bounded wire contracts. Each option-discovery request is limited to 4,096 bytes and each option-discovery response to 262,144 bytes. Native-create requests are limited to 65,536 bytes; create responses and create-status requests to 4,096 bytes; and create-status responses and rename requests and responses to 16,384 bytes. Requests and responses close over authenticated caller and destination identities plus the exact request or operation and target identities. A peer that does not implement a required endpoint fails explicitly. No compatibility alias, generic remote CRUD/settings surface, trust or enrollment authority, or lifecycle authority is introduced.

## Consequences

- Local HTTP, local native tools, and federation destination handlers converge on the same typed application actions.
- Remote creation uses destination-scoped options while retaining the existing local intent and retry identity.
- Retry after an uncertain response is safe: identical intent reconciles the original durable result and changed intent conflicts.
- Retained creation jobs and immutable rename receipts require a constrained peer-qualified provenance shape plus a migration that conditionally backfills complete historical native call locators and preserves provenance-absent creation rows.
- Federation adapters remain responsible only for selection, authenticated transport, strict decoding, identity closure, and conversion to shared action inputs/results.
- Unsupported or mixed-version peers fail without fallback; no downgrade or cross-version compatibility guarantee is added.

## References

- `specs/federation/requirements.md` — REQ-FED-021 through REQ-FED-024
- `specs/federation/federation.allium`
- `specs/global-recall/requirements.md` — REQ-GR-024
- `specs/conversation-creation/requirements.md` — REQ-CCR-013
- `specs/conversation-identity/requirements.md` — REQ-CID-007
- `specs/compatibility/requirements.md`
- [ADR-191](191_native-create-and-rename-use-shared-application-actions.md)
