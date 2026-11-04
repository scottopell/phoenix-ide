# ADR-218: Reminder occurrence identities are reserved at fresh input admission

- **Status:** Accepted
- **Date:** 2026-11-03
- **Affects:** REQ-REM-005, REQ-REM-008, REQ-API-033

## Context

Scheduled reminders use deterministic occurrence identities in the ordinary direct-turn and steering journals. Public chat also accepts caller-selected identities. A caller can learn a reminder ID and preclaim its occurrence key with unrelated input. Key-only discovery then mistakes that input for reminder acceptance; filtering discovery alone leaves the key occupied and ordinary admission conflicts forever.

Steering acceptance outlives its live queue entry. Its exact request fingerprint proves replay equality, but cannot be decoded into scheduled provenance after queue removal. Continuation can move an acceptance receipt while retaining its original fingerprint.

## Options considered

1. Filter only live input rows by origin. This cannot identify removed steering acceptances and does not prevent key occupation.
2. Reconstruct generic request fingerprints during reminder discovery. This couples scheduling to canonical request encoding and continuation replay rules.
3. Reserve the namespace at fresh standard input admission and retain reminder source identity on the existing steering acceptance receipt.

## Decision

Choose option 3. Shared Send Chat rejects new non-reminder inputs whose raw client identity begins with `reminder:`. Exact accepted replay is checked first, preserving historical ordinary client replay. Trusted scheduled inputs retain the existing due-state, owner, exact key, text, attachment and policy validation.

Direct-turn discovery requires the matching scheduled origin. Steering discovery requires a matching reminder ID on the existing acceptance receipt and an exact fingerprint. The receipt retains that provenance when its queue entry is copied, removed or continued. Generic replay continues to use its existing fingerprint unchanged. No acceptance flag or separate journal is added to reminder rows.

Migration 151 is unreleased and adds the nullable provenance column. Pre-feature ordinary receipts have no reminder source; NULL preserves that true absence. No historical scheduled receipt backfill or additional compatibility guarantee is introduced.

## Consequences

- **Positive:** Public callers cannot occupy a reminder occurrence key, and unrelated journal rows cannot suppress a due occurrence.
- **Positive:** Removed and continued steering acceptances remain recognizable without reconstructing live queue state or request hashes.
- **Negative:** New arbitrary client identities in the `reminder:` namespace are refused; existing exact replay remains available.

## References

- ADR-219 extends the reservation to canonical aliases and records the existing-owned-opening admission boundary.
- ADR-217: reminder submission through standard input admission.
- `../reminders/requirements.md`, `../api/requirements.md`.
