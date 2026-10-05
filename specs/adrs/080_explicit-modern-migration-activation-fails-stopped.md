# ADR-080: Explicit modern migration activation fails stopped

- **Status:** Accepted
- **Date:** 2026-10-05
- **Affects:** launchd deployment, production deployment, compatibility

## Context
The ordinary launchd deployment helper restores runtime artifacts and starts the predecessor after candidate failure. That is insufficient for schema112→113 migration delivery: an older runtime must not open candidate-mutated SQLite before a matching offline restore. The prepared ProductConversation upgrade contract is intentionally legacy-only; widening its eligibility would imply an unqualified automatic database recovery guarantee. The commissioned delivery policy calls for the smallest separate opt-in, not a recovery platform or RC1 amendment.

## Options considered

1. Reuse ordinary artifact rollback: rejected because it starts an older runtime against candidate-mutated SQLite before manual restoration.
2. Widen the legacy paired automatic database recovery eligibility: rejected as a new compatibility/recovery guarantee outside this scope.
3. Explicit stopped/fenced ordinary activation with verified offline backup and manual restore: selected; keeps recovery offline and preserves normal runtime-only behavior.

## Decision
Expose `prod deploy --migration-backup-receipt PATH` for explicitly selected ordinary macOS launchd local-HEAD or published-release source. The service must already be stopped. Admit only exact installed configuration, captured predecessor artifacts and a private schema1 offline backup/rehearsal receipt. The byte-bound selected-source helper rechecks offline exclusivity, modern migration ledger, integrity, and logical schema/all-row equivalence between stopped source and retained backup/rehearsal; operator-filled hashes alone do not establish source equivalence. Sidecars must be checkpointed safely by the operator before admission, never blindly discarded.

After candidate disruption, any activation failure leaves the service stopped/quarantined with retained ownership and actionable diagnostics; no automatic predecessor bootstrap or database restore occurs. Manual offline matching database restore is the operator boundary. `prod resume-migration TXN` uses the retained helper, verifies helper absence/lock/claim, stopped database bytes and logical contents against the backup, then resumes only captured predecessor artifacts/configuration. Resume failure retains the fence. A recorded predecessor-start checkpoint refuses startup replay; the helper never replays a database restore.

Ordinary deployments without this option retain existing runtime-artifact rollback. Legacy paired eligibility and publication-only committed finalization remain separate. This policy is macOS launchd-only, does not accept arbitrary prepared artifacts, and does not grant generic downgrade or Linux recovery.

## Consequences
Migration failure has a safe offline recovery point, at the cost of explicit operator backup/checkpoint/rehearsal work and possible downtime. Backups remain private, disk capacity is an admission responsibility, and the helper's logical equivalence scan can be expensive for large databases. The receipt is local evidence, not release provenance. Retained resume-start uncertainty refuses automatic replay rather than guessing whether an old runtime accepted later writes. No production operation is authorized by this source change.
