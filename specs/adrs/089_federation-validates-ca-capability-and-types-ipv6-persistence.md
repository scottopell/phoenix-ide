# ADR-089: Federation validates CA capability and types IPv6 persistence

- **Status:** Accepted
- **Date:** 2026-10-09
- **Affects:** REQ-RQD-003, REQ-RQD-004

## Context

ADR-088 makes imported private-CA trust explicit and peer-scoped. Parsing one certificate and adding it to a rustls root store proves neither that the certificate asserts certificate-authority capability nor that a present Key Usage extension permits certificate signing. Accepting a leaf or a CA forbidden from signing certificates would persist unusable trust and defer the failure until a remote query.

Peer origins also need to represent IPv6. The peer table stores domain and IPv4 hosts as text under a character constraint that deliberately rejects colons. Widening that text constraint would admit ambiguous or malformed colon-bearing values and would not update databases that already applied the migration which created the table.

## Options considered

1. **Validate CA extensions and add a typed IPv6 persistence branch** — reject certificates without CA Basic Constraints or with a present Key Usage that forbids certificate signing; preserve domain/IPv4 text and store IPv6 as exactly 16 bytes in a disjoint column.
2. **Rely on rustls and store every host as text** — smaller source change, but unusable trust reaches persistence and IPv6 syntax remains weakly constrained.
3. **Reject IPv6 peer origins** — preserves the existing table shape but excludes valid HTTPS origins and mismatches URL/TLS capabilities.

## Decision

Validate imported private-CA certificates before persistence. Basic Constraints must be present and identify a certificate authority. When Key Usage is present, it must permit certificate signing. Duplicate or malformed relevant extensions fail closed.

Persist peer hosts as a relational sum type. A row contains either a domain/IPv4 text host or an IPv6 host encoded as exactly 16 bytes, never both or neither. Migration 120 rebuilds the peer table, classifies existing host rows as domain/IPv4, and preserves the complete peer connection. URL brackets are reconstructed only when creating an HTTPS authority.

This keeps certificate capability failure at the enrollment boundary and makes invalid IPv6 persistence states structurally unrepresentable without changing ADR-088's per-peer trust choice.

## Consequences

- **Positive:** Imported leaf certificates and CA certificates forbidden from signing fail before persistence or network use.
- **Positive:** IPv6 peers round-trip canonically without ambiguous bracket or colon handling.
- **Positive:** Existing peer rows migrate without changing their origin or trust semantics.
- **Negative:** Certificate admission requires X.509 extension parsing in addition to rustls trust-anchor parsing.
- **Negative:** Peer host persistence and its migration use two nullable columns plus an exclusive-or constraint.
- **Neutral:** Domain and IPv4 validation remains governed by `PeerBaseUrl`; the schema retains its prior bounded character constraint.

## References

- ADR-088
- ADR-083
- ADR-034
- `PeerCaCertificatePem::parse`
- `PeerBaseUrl::host`
- `Database::save_federation_peer_connection`
- `MIGRATION_120`
- `specs/remote-query-database/requirements.md`
