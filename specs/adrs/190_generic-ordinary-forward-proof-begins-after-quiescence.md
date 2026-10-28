# ADR-190: Generic ordinary forward proof begins after quiescence

- **Status:** Accepted
- **Date:** 2026-10-27
- **Affects:** REQ-COMP-019; REQ-LDD-028; native launchd ordinary local-HEAD and published-release deployment with pending migrations

## Context

ADR-148 qualified one exact ordinary launchd source/target interval. That exception proved the forward-only lifecycle, but exact interval pins do not scale to every additive migration suffix whose live ledger is already a complete ordered prefix of the candidate registry.

REQ-COMP-019 and REQ-LDD-028 define the generic qualification: immutable candidate evidence and an exact live-ledger prefix are admitted before disruption; the service is then stopped, exclusive database ownership is proved, and the exact candidate performs the pending suffix behind the existing pre-invocation/forward-only boundary.

The live predecessor remains authoritative until it is stopped. It may therefore commit legitimate application data after pre-disruption admission. A digest of all logical database contents captured during admission cannot also be the proof that the database remained unchanged after stop. Treating it as that proof can reject a safe pre-invocation rollback solely because the predecessor completed accepted work before quiescence.

## Options considered

1. **Continue adding exact source/target pins** — rejected because exact pins duplicate the same proof and recovery lifecycle for every additive suffix and make compatibility depend on release-by-release code changes rather than complete ledger evidence.
2. **Use the admission-time logical digest as the rollback baseline** — rejected because the predecessor can legitimately write between admission and stop, so equality to that digest is not evidence of post-quiescence mutation.
3. **Bind the exact-prefix source after stop and exclusivity** — selected because it admits legitimate pre-stop writes while creating a stable source baseline before migration invocation.
4. **Freeze writes or require a standalone database snapshot before admission** — rejected because the service stop and exclusive-ownership boundary already creates the point at which logical contents can be bound. Adding a snapshot or an earlier write freeze would expand the ordinary protocol into the protected deployment protocols without a corresponding recovery guarantee.

## Decision

Phoenix uses one generic exact-prefix ordinary launchd protocol for a non-empty pending migration suffix:

1. Before disruption, the retained helper validates the complete ordered live ledger as an exact prefix of the immutable complete candidate registry and validates all transaction-bound candidate, configuration, helper, path, hash, integrity, and ownership evidence required by REQ-LDD-028. A non-prefix or unavailable ledger rejects at this stage.
2. The admission observation decides whether the generic forward protocol applies; it does not freeze ordinary application data while the predecessor is still running.
3. After the predecessor is observed stopped and exclusive database ownership is proved, the helper re-reads the complete ledger and records a logical database digest. This stopped observation is the durable source baseline for the transaction.
4. Immediately before the candidate migration invocation, the helper revalidates the ledger, registry, and logical digest against that stopped baseline. Only equality to this baseline proves that the pre-invocation database remains unchanged and can authorize restoration of the captured predecessor artifacts.
5. The durable invocation checkpoint remains the irreversible boundary. Once invocation can have advanced the database, only the exact retained candidate may continue; predecessor restart and inferred database rollback are prohibited.

The exact source-129-to-target-131 transition covered by ADR-148 is an instance of this generic protocol, not a separate ongoing code path.

## Consequences

- **Positive:** Invalid migration ledgers fail before runtime disruption; legitimate writes completed before stop become part of the bound source state and do not prevent a safe pre-invocation artifact rollback.
- **Negative:** Ordinary deployment gains no standalone backup, database rollback, downgrade, protected-route rehearsal, alternate backend, or cross-version predecessor compatibility guarantee.
- **Neutral:** A mutation after the stopped baseline fails closed and prevents predecessor restart. Durable evidence distinguishes admission from the stopped rollback baseline; recovery never reconstructs this authority from a stale pre-stop digest.

## References

- `specs/compatibility/requirements.md`: REQ-COMP-013, REQ-COMP-019
- `specs/launchd-deployment/requirements.md`: REQ-LDD-023, REQ-LDD-028
- `specs/launchd-deployment/launchd-deployment.allium`: ordinary forward admission, start-attempt, rollback, and forward-only rules
- Prior decision: ADR-148
