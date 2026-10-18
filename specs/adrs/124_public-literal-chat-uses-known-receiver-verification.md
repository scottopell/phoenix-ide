# ADR-124: Public literal chat uses known-receiver verification

- **Status:** Accepted
- **Date:** 2026-10-17
- **Affects:** REQ-API-031; built-in `PhoenixApiDestinationAuthority` guidance

## Context

The shared chat application service supports literal input internally, but public chat expands references. User-authorized API steering needs a small public literal contract without exposing generated context or bypassing authentication, attachment validation or lifecycle admission. During review, the user explicitly selected the existing `/chat` policy field and a minimal controlled-destination bootstrap, rejecting extra endpoints, capability flags and negotiation machinery.

Historical receivers can ignore an unfamiliar request field. A harmless HTTP-success smoke cannot prove policy semantics; matching transcript text alone also does not prove which expansion policy was accepted historically.

## Options considered

1. **Existing `/chat` policy field with known-receiver verification** — smallest public surface; requires independently verified source identity at the selected destination.
2. **Dedicated literal route and version capability** — rejected as unnecessary endpoint and negotiation surface.
3. **General protocol negotiation or receiver fencing** — rejected as broader lifecycle/compatibility machinery than the authorized use needs.

## Decision

Expose only the closed `ChatExpansionPolicy` enum (`expand_references`, `literal_text`) on existing `/chat`, preserving the expanding default. Add no route or version response field.

Use a five-step recipe: establish destination/auth/TLS; verify complete `/api/version` JSON with full `git_sha` exactly matching an immutable supporting commit independently inspected or a source-inspected accepted descendant; confirm one harmless `literal steering smoke test` on the exact authorized idle target with retained ID and materialized bytes; send explicit literal text; observe/reconcile with exact retained intent. Re-read version immediately before use and each retry. Unknown or mismatched receivers stop before POST, without expandable probes or default fallback. Examples use placeholders and authoritative commit inspection, not a self-referential hardcoded branch commit.

Require durable policy evidence for nondefault replay. Direct-turn receipts and exact steering fingerprints remain authoritative; transcript-only historical evidence can replay only default expanding intent. No new receipt framework or database migration is added.

## Consequences

- **Positive:** Literal idle and steering sends reuse existing authorization, admission, attachment handling, provenance and idempotency without extending endpoint or negotiation surface.
- **Negative:** Operators must verify a known receiver before sending expandable content. This preflight does not fence concurrent binary replacement, and smoke success alone cannot prove policy support. Receipt-free historical literal retries conflict even when visible text matches.
- **Neutral:** No database downgrade, historical binary support, mixed-version ownership, deployment or live-replacement guarantee is added. Local-only tool and destination-authority boundaries are unchanged.

## References

- [API requirements](../api/requirements.md), REQ-API-031
- [Compatibility requirements](../compatibility/requirements.md), REQ-COMP-001
- [Built-in guidance](../builtin-skills/builtin-skills.allium), `PhoenixApiDestinationAuthority`
- ADR-034 (explicit compatibility policy), ADR-113 (non-exhaustive API recipes)
- `ChatRequest`, `ChatExpansionPolicy`, `send_chat`, `SendChatApplicationService`, `lookup_durable_steering_replay`
