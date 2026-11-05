# ADR-216: Explicit local tool selection reuses local admission

- **Status:** Accepted; remote-SQL public tool shape partially superseded by ADR-222
- **Date:** 2026-11-02
- **Affects:** REQ-FED-001, REQ-FED-002, REQ-FED-019

## Context

The Coordinator repeatedly supplies `instance: "local"` while trying to message local conversations. Treating every supplied selector as a remote peer name rejects these calls, including a handoff asking another agent to repair the same problem. The user asks to support the reasonable tool argument instead of relying on prompt corrections, and to disallow the confusing name local for peers.

## Options considered

1. Keep omission as the only local selector and strengthen tool instructions. This preserves the input contract but does not resolve the observed repeated failures.
2. Treat the exact string `local` as local at the optional-instance tool boundary and allow peers with that name only through UUID selection. This preserves local services but leaves a confusing peer label.
3. Make peer lookup fall back to this instance for unknown names or self UUIDs. This accepts more inputs but risks executing an intended remote action locally.
4. Reserve the selector and reject local as a new or replacement peer name. This removes the confusing label while preserving ordinary remote selection and failure behavior.

## Decision

Use one shared typed local/remote selector for all four optional-instance tools: message send, conversation read, reference resolution, and Goal read. Omission and exact `"local"` select the same local variant before any remote-only validation or peer lookup. Preserve each tool's explicit-null behavior. All other strings retain saved-peer selection and its failure behavior; no fuzzy local aliases or remote-to-local fallback are introduced.

Reserve the exact literal before peer-name lookup. Enrollment and saved-connection creation/replacement require a validated `PeerDisplayName`, which trims the name and rejects blank values and local independent of ASCII case. The API validates the name before any credential or connection mutation, and the database write services require the validated type. Existing stored labels remain read data; no migration, silent renaming, enrollment, credential or trust change is performed by tool selection. The required UUID input of remote-only SQL is unchanged.

## Consequences

- **Positive:** Repeated explicit-local calls use the existing local admission, provenance, bounds, result and retry paths.
- **Positive:** Remote selection failures still cannot execute an operation locally.
- **Negative:** An owner must choose a different display name when creating or replacing a peer named local, including case/whitespace variants.
- **Neutral:** Tool selection recognizes only exact lowercase `local`; other selectors retain remote resolution, while peer-name admission independently excludes confusing names.

## References

- [Federation requirements](../federation/requirements.md), REQ-FED-001–002 and REQ-FED-019.
- [Federation behavior](../federation/federation.allium), `SelectLocalTool`, `SelectRemoteTool`, and `RemoteSelectionFails`.
- `coordinator_tools::instance::InstanceSelector` and `api::federation::resolve_saved_peer_id`.
- `instance_identity::PeerDisplayName`, `Database::replace_federation_enrollment`, and `Database::save_federation_peer_connection`.
