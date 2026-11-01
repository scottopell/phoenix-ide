# ADR-094: Owner and peer credential domains are structurally disjoint

- **Status:** Accepted
- **Date:** 2026-10-12
- **Affects:** REQ-AUTH-002, REQ-AUTH-009; `OwnerPassword`, `PeerBearerCredential`

## Context

Phoenix accepts an operator-selected `PHOENIX_PASSWORD` as owner authority and mints receiver-issued federation bearers as peer authority. Both arrive in the same HTTP Bearer carrier. If the same raw string can be valid in both domains, request context cannot reveal which authority the presenter intended, so exact owner-password comparison and mandatory peer rejection cannot both hold.

The peer credential format already has a complete typed shape: `phx_peer_` followed by 43 URL-safe base64 characters. Owner passwords may legitimately begin with the same prefix without occupying that complete shape.

## Options considered

1. **Reject every owner password beginning with `phx_peer_`** — simple, but rejects values outside the actual peer credential domain and contradicts exact configured-password behavior unnecessarily.
2. **Query active enrollment verifiers during owner authentication** — rejects known collisions, but makes owner authority depend on mutable database contents, leaves inactive or remote peer bearers representable as owner passwords, and adds persistence failure semantics to every password check.
3. **Make owner and peer credential types disjoint at configuration admission** — reject only the complete peer bearer shape before serving requests while preserving all other owner-password values and ordinary constant-time comparison.

## Decision

Adopt disjoint typed credential domains. `OwnerPassword` rejects the complete `PeerBearerCredential` shape at startup. `PeerBearerCredential` continues to accept only that reserved shape. A `phx_peer_` prefix by itself does not reserve an owner password.

Owner authentication compares every admitted owner password by exact constant-time equality. Peer authentication remains independently bound to an active persisted enrollment verifier. No stored or issued credential is rewritten or rotated by this validation.

## Consequences

- **Positive:** One raw bearer cannot grant both owner and peer authority, so peer rejection and exact owner-password verification are simultaneously enforceable.
- **Positive:** Owner passwords that merely begin with `phx_peer_` remain valid.
- **Negative:** An operator configuration that exactly matches the reserved peer bearer shape fails startup and must be changed before Phoenix serves requests.
- **Neutral:** Existing federation bearer issuance, verifier persistence, replacement, and revocation formats do not change.

## References

- ADR-083, ADR-034
- `OwnerPassword::parse`, `PeerBearerCredential::parse`, `owner_password_matches`
- [Auth requirements](../auth/requirements.md) and [Auth Allium specification](../auth/auth.allium)
