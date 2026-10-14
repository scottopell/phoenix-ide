# ADR-105: Ordinary forward candidate launch is the downgrade boundary

- **Status:** Accepted
- **Date:** 2026-10-14
- **Affects:** REQ-COMP-013, REQ-LDD-023, REQ-PD-022

## Context

Ordinary `dev.py prod deploy` performs an in-place forward activation without a standalone database backup. Before this decision, any candidate startup or health failure caused the helper to restore and launch the predecessor binary and plist while leaving the database as the candidate had written it.

That is unsafe for the migration-123 to migration-127 transition. Migration 126 commits its retirement metadata and ledger entry in one transaction independently of later migrations; predecessor wake queries do not understand that metadata and predecessor writes can be rejected by the new retirement triggers. Migration 127 changes the schema, and subsequent coordinator-home initialization commits a new work-scope environment kind before later recovery and admission. The predecessor resource-authority reader rejects that kind. Failed health does not prove which writes committed, and an unchanged highest ledger does not prove initialization did not commit.

## Options considered

1. **Always restore the predecessor runtime** — preserves prior availability behavior, but can run an incompatible old binary after migration or initialization writes commit.
2. **Require a standalone database backup for every ordinary deployment** — enables a paired rollback story, but broadens normal deployment policy and adds a capacity/outage prerequisite the ordinary route does not need.
3. **Make candidate startup a forward-only boundary** — keeps ordinary deployment backup-free and preserves pre-start rollback, while accepting a fenced outage when candidate startup becomes uncertain.

## Decision

Do not add a universal backup requirement to ordinary in-place deployment.

For the exact migration-123 baseline and migration-127 candidate registry, retain the candidate registry and helper as immutable transaction evidence. Failures before candidate startup is attempted remain eligible for the existing predecessor runtime rollback because the candidate could not have mutated the database.

Persist a forward-only boundary immediately before the first candidate startup attempt. Any failure or uncertainty after that point stops the candidate when possible, retains the candidate binary/plist and deployment claim, and forbids predecessor launch. The helper does not guess from the ledger, health result, or production row population.

Recovery is an explicit operator decision through `prod resume-forward TXN`. That command verifies the retained helper, candidate binary/plist, registry, stopped target, and claim, then starts only the exact retained forward-compatible candidate. Another failure remains fenced and forward-only.

## Consequences

- Normal in-place deployment remains backup-free.
- A failed forward activation can produce a deliberate outage rather than an unsupported mixed predecessor/database pair.
- Runtime rollback remains available for pre-start failures and for other ordinary deployments outside the exact qualified boundary.
- No automatic database rollback or predecessor-on-new-schema compatibility guarantee is created.
- Operators need an explicit forward-recovery authorization after the boundary is crossed.

## References

- `specs/compatibility/requirements.md` REQ-COMP-013
- `specs/launchd-deployment/requirements.md` REQ-LDD-023
- `specs/production-deployment/requirements.md` REQ-PD-022
- ADR-104
