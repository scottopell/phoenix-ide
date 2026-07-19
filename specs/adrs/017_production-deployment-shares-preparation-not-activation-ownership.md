# ADR-017: Production deployment shares preparation but keeps backend-owned activation

- **Status:** Accepted
- **Date:** 2026-07-15
- **Affects:** REQ-PD-001 through REQ-PD-017

## Context

Phoenix supports native production operation through macOS launchd, Linux systemd, and a fallback for Linux hosts without systemd. Every deployment source needs the same identity and artifact verification, but activation crosses different ownership and privilege boundaries. Phoenix may terminate while replacing itself, systemd installation requires a narrow privileged path, and a Linux host without an init service needs a persistent same-user process to own Phoenix across initiating-shell failure.

The existing launchd transaction proves that preparation can finish before disruption and hand activation to an external owner. Extending that safety to Linux requires deciding whether to duplicate the full protocol per backend or share the preparation and transaction model.

## Options considered

1. **Keep three independent deployment implementations** — preserves local simplicity, but candidate verification, status, claims, release handling, and rollback semantics can diverge silently.
2. **Use one generic activation helper and an untyped backend metadata map** — maximizes shared code, but makes backend capability and privilege differences runtime conventions rather than structural contracts.
3. **Share typed preparation and transaction semantics while keeping typed backend-owned activation** — centralizes source and identity guarantees while representing launchd, systemd, and bare-Linux ownership explicitly.

## Decision

Use one typed preparation layer for local and published candidates, immutable transaction inputs, runtime identities, claims, durable status, and terminal outcomes. Use structurally distinct backend handoffs and activation owners: a one-shot LaunchAgent for launchd, a root-owned transient unit for systemd, and a persistent same-user supervisor for bare Linux.

The systemd privileged boundary accepts only validated inputs copied into a root-owned transaction location and restricts units, users, data paths, and installation roots. The bare-Linux supervisor accepts only immutable transaction references over owner-only IPC and directly parents Phoenix. `.phoenix-ide.env` is snapshotted once as the modern configuration source; backend-specific override stores are not part of the modern protocol, and `prod set`/`prod unset` provide migration guidance rather than mutating configuration.

ADR-010 remains the historical decision for launchd's independent helper. This decision generalizes the shared contract without changing launchd's platform-specific ownership rationale.

## Consequences

- **Positive:** local and published deployment sources converge on one checksum and exact-identity protocol across every backend.
- **Positive:** invalid cross-backend handoffs and arbitrary privileged target paths can be rejected structurally rather than by an untyped metadata convention.
- **Positive:** Phoenix and the initiating shell are outside the post-handoff recovery path on every backend.
- **Negative:** the implementation maintains three activation owners and their disposable integration environments.
- **Negative:** systemd requires a carefully constrained root boundary, while bare Linux requires supervisor installation, IPC, child reconciliation, and an explicitly limited reboot-persistence promise.
- **Neutral:** same-UID processes are inside the bare supervisor's trust boundary; owner-only IPC protects against other users and stale or malformed clients, not a hostile process with the same Unix identity.

## References

- ADR-010
- `specs/production-deployment/requirements.md`
- `specs/production-deployment/production-deployment.allium`
- `specs/launchd-deployment/requirements.md`
- `launchd_prod_deploy`
- `native_prod_deploy`
- `prod_daemon_deploy`
