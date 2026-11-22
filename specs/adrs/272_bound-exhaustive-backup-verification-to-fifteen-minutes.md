# ADR-272: Bound exhaustive backup verification to fifteen minutes

- **Status:** Accepted
- **Date:** 2026-11-22
- **Affects:** REQ-LDD-029, REQ-LDD-031
- **Qualifies:** [ADR-263](263_verify-native-backup-and-bound-predecessor-recovery-for-macos-deploy.md)

## Context

A retained production-shaped SQLite diagnostic snapshot contained 13,745,197,056
bytes, 6,988,720 compared rows and 63,467,170 compared cells. Full stopped-source
verification under the ten-minute policy did not complete. A human-approved
fifteen-minute experiment with the original comparator also failed: proof hashing
had started but had not completed by the deadline. A subsequent bounded diagnostic
measured a complete unchanged proof-hash operation separately; that measurement
was not retroactive qualification.

Repeated storage-tag decoding, primitive coercion and tuple allocation in the
per-cell comparator were avoidable without changing SQLite storage-class or value
semantics. Adversarial differential fixtures and independent source review covered
TEXT/BLOB bytes, numeric storage classes, binary64 signed zero, schema, rowid,
WITHOUT ROWID keys and FTS shadow contents.

## Options considered

1. Keep the ten-minute verification limit and accept that this workload cannot
   qualify for a deployment attempt.
2. Weaken integrity, logical-equivalence or proof verification, or infer success
   by combining measurements from different failed runs.
3. Use a shared fifteen-minute verification deadline with only the
   semantics-preserving comparator allocation reduction, while preserving the
   separate ten-minute candidate-migration limit and all existing safety checks.

## Decision

Choose option 3. Native copy, integrity checking, metadata and exhaustive typed
logical comparison, destination fsync and proof hashing share one absolute
900-second verification deadline. Candidate migration retains its independent
600-second limit. Verification failure still prevents installation or migration;
writer reservation, transaction-matched durable proof, predecessor recovery and
postboundary fail-stop rules remain unchanged.

The combined helper passed one independent-image full-path isolated experiment:
817.564 seconds in the worker and 817.972 seconds in its supervisor, including
proof hashing. Native copy took 83.774 seconds, integrity 403.330 seconds, metadata
0.081 seconds, comparison 316.863 seconds, fsync less than 0.001 seconds and proof
hashing 13.510 seconds. The experiment used CPython 3.13.1 and SQLite 3.47.1. These
are observations, not a guaranteed runtime or a controlled estimate of the
comparator improvement; earlier same-file comparison measurements differed in
file/cache topology and preceding work.

## Consequences

- The allowed stopped verification interval is longer. Synchronous OS I/O is not
  made interruptible by a Python deadline, and this is not a hard outage bound.
- No check is sampled or weakened to meet the deadline. A censored or incomplete
  proof is failure, not a reusable verified backup.
- Isolated success does not admit a live rollout. Exact-source review, hosted
  workload qualification, release verification and fresh rollout/resource gates
  remain independent evidence.
- This decision does not authorize another budget increase, a new live snapshot,
  automatic postboundary rollback, or replacing an already published release.

## References

- [macOS launchd deployment requirements](../launchd-deployment/requirements.md)
- [macOS launchd deployment status](../launchd-deployment/executive.md)
- [Compatibility policy](../compatibility/requirements.md)
