# ADR-060: Modern build identity is full-length and legacy rollback is role-bound

- **Status:** Accepted
- **Date:** 2026-09-19
- **Affects:** REQ-DEPLOY-002; REQ-PD-002, REQ-PD-009, REQ-PD-010, REQ-PD-014; REQ-LDD-007, REQ-LDD-008, REQ-LDD-011, REQ-LDD-016; `RuntimeIdentity`, `DeployTransaction`

## Context

Phoenix uses an embedded git commit as both an operator-visible build fact and a security-relevant deployment identity. A 12-character commit prefix is compact, but prefix matching delegates uniqueness to repository history and makes a modern candidate's identity less precise than the immutable 40-character commit selected by deployment and release publication.

Production activation also stages the already-installed runtime as the rollback predecessor. Installations created before full-length embedding can report a 12-character identity even though their durable deployed source record is a full commit. Refusing that captured predecessor would remove automated recovery during an otherwise modern candidate activation; accepting shortened identities generally would instead create an open-ended controller and downgrade compatibility promise.

Bare Linux adds a separate persistent-authority boundary. The supervisor outlives Phoenix and owns activation after handoff. Two supervisor programs can report the same protocol version while differing in validation or recovery behavior, so protocol equality alone cannot prove that a running supervisor is the selected activation program.

Automated deployment rollback restores runtime artifacts but does not restore the SQLite database. Identity compatibility must not turn that bounded recovery operation into a claim that an older binary can use data changed by a candidate.

## Options considered

1. **Keep 12-character embedded identities everywhere** — preserves compact values and existing prefix comparisons, but makes candidate identity dependent on prefix uniqueness and carries avoidable ambiguity through APIs and deployment protocols.
2. **Accept both 12- and 40-character identities throughout deployment** — eases transitions between binary and controller versions, but creates general cross-version compatibility, downgrade, and controller-interchangeability obligations.
3. **Require full exact identity for modern builds and candidates, with a predecessor-only legacy allowance and fail-closed persistent supervision** — makes all newly selected artifacts exact while preserving one bounded recovery path for the already-installed rollback artifact.

## Decision

Choose option 3.

A known modern Phoenix build embeds its full 40-character lowercase git commit SHA. Deployment and version APIs retain that complete value. Presentation surfaces may render a 12-character prefix for visual economy only when the full identity remains available in title and accessibility text.

Every modern local or published deployment candidate must report a clean full embedded SHA equal to the selected full source commit. Release packaging, candidate preparation, activation manifests, runtime verification, and newly written deployed identity use exact full equality; a prefix match does not identify a modern candidate.

A transaction may accept a 12-character lowercase SHA only as the captured identity of the already-installed runtime staged as its rollback predecessor. That predecessor identity may instead be the modern 40-character form. The allowance exists solely to verify restoration after that transaction's failed activation. It does not admit shortened candidate, release, helper, controller, or installed-restart identity and does not establish a general downgrade or cross-version deployment protocol.

A running bare-Linux supervisor is persistent activation authority, not an interchangeable protocol endpoint. When its installed artifact differs byte-for-byte from the supervisor selected by a deployment, deployment fails before disruption even if protocol versions match. Phoenix does not replace, restart, or reuse that changed running supervisor as a compatibility bridge.

Successful automated recovery is reported as runtime-artifact rollback. It restores and verifies the captured predecessor's runtime artifacts, configuration, environment, service state, endpoint, and deployed source record. It does not restore a database and does not guarantee that the restored binary can use a database changed by the candidate. Manual version rollback and database recovery remain governed by the offline paired-restore contract in `specs/compatibility/requirements.md`.

## Consequences

- **Positive:** Modern build and candidate identity is one unambiguous value across embedding, APIs, release assets, manifests, health verification, and durable deployed state.
- **Positive:** Compact UI presentation remains available without discarding full machine-readable or accessible identity.
- **Positive:** An already-installed legacy runtime remains available as the immediate transaction predecessor without making legacy identity a generally accepted input.
- **Positive:** A changed persistent bare supervisor cannot silently execute a newer controller's transaction under an insufficient protocol-version check.
- **Negative:** A modern candidate with a shortened, uppercase, dirty, or merely prefix-matching identity is rejected even when its source appears inferable.
- **Negative:** Updating a running bare supervisor whose bytes differ requires an explicit external stop before deployment can install and start the selected supervisor.
- **Neutral:** Runtime-artifact rollback can restore process operation yet still require offline operator recovery if the candidate changed the database incompatibly.

## References

- ADR-010: launchd deployment uses an independent transaction helper.
- ADR-017: production deployment shares preparation but keeps backend-owned activation.
- ADR-034: compatibility guarantees are explicit and data-aware.
- `specs/deployment-info/requirements.md`
- `specs/production-deployment/requirements.md`
- `specs/production-deployment/production-deployment.allium`
- `specs/launchd-deployment/requirements.md`
- `specs/launchd-deployment/launchd-deployment.allium`
- `BuildInfo`, `RuntimeIdentity`, `_prepare_local_candidate`, `_prepare_release_candidate`, `_rollback_identity_matches`, `_start_bare_supervisor`
