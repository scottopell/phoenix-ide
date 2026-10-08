# ADR-088: Federation pins imported private-CA trust per peer

- **Status:** Accepted
- **Date:** 2026-10-07
- **Affects:** REQ-RQD-003, REQ-RQD-004; federation enrollment transfer and caller-side peer persistence

## Context

Phoenix supports ordinary publicly trusted TLS certificates and managed certificates signed by a Phoenix-local private certificate authority. Federation clients must retain certificate-chain and hostname verification without requiring an operating-system trust-store edit. The directional enrollment transfer is already an owner-authorized, out-of-band exchange of peer identity and a bearer credential, so private trust needs an equally explicit bootstrap authority.

Managed leaf certificates rotate under a stable local CA. Pinning a leaf would therefore break ordinary renewal, while installing a CA globally would expand one peer's authority to unrelated clients and services.

## Options considered

1. Use only platform roots: simple, but managed Phoenix certificates cannot connect without an undocumented host trust-store mutation.
2. Disable certificate or hostname verification: operationally convenient, but removes peer authentication from TLS.
3. Pin each peer's public CA in its persisted connection: supports managed renewal while limiting the added trust root to one selected peer client.
4. Pin leaf certificates: narrower than CA trust, but incompatible with routine managed leaf renewal.

## Decision

Represent peer TLS trust as an exhaustive choice between platform roots and exactly one bounded public private-CA certificate. Include that choice in the owner-authorized enrollment transfer and persist it atomically with peer identity, origin, and bearer credential. Construct a short-lived HTTP client for the selected peer and add only that peer's private CA to its roots. Retain certificate-chain, validity, hostname, HTTPS-only, and no-redirect enforcement.

A private CA is a trust anchor, not a credential: never transfer or persist its private key. Replacing a peer connection may explicitly replace its trust anchor; there is no silent trust-on-first-use or automatic CA rotation protocol. Existing peer rows without a private CA continue to mean platform-root validation.

## Consequences

- Phoenix-managed peers connect without operating-system trust-store edits or verification bypasses.
- A peer-specific CA cannot expand trust for other federation peers or unrelated HTTP clients.
- Leaf renewal under the same CA remains valid; CA rotation requires explicit owner re-import or re-enrollment.
- Enrollment bundles are sensitive bootstrap artifacts because they carry both a bearer credential and the trust decision needed to authenticate its receiver.
- Migration-era peer rows retain their original platform-root behavior rather than gaining an inferred private trust anchor.

## References

[Remote database query requirements](../remote-query-database/requirements.md), [authentication requirements](../auth/requirements.md), [compatibility requirements](../compatibility/requirements.md), ADR-034, ADR-083
