# ADR-019: Runtime ownership requires positive evidence

- **Status:** Accepted
- **Date:** 2026-07-18
- **Affects:** REQ-DEPLOY-002A, REQ-RU-004A

## Context

Phoenix reports who manages the running process and uses that answer to select a
disruptive release-update backend. The host platform, PID 1, and installed
service artifacts describe installation capability or historical state, but do
not prove which owner launched the current process. A plausible guess can route
an update through the wrong activation and rollback authority.

## Options considered

1. **Infer ownership from host characteristics** — simple and aligned with the
   default deployment choice, but mislabels manual processes and non-default
   supervisors.
2. **Infer ownership from installed artifacts** — identifies configured service
   managers, but stale units, plists, and sockets can outlive their ownership.
3. **Require positive runtime evidence** — use listener provenance for launchd
   and systemd, and an authenticated direct-parent protocol claim for the bare
   supervisor; preserve uncertainty when evidence is absent or contradictory.

## Decision

Phoenix requires positive runtime evidence before reporting a managed owner or
allowing disruptive in-app updates. Launchd and systemd ownership comes from the
socket-activation contract consumed by the process. Bare ownership requires an
owner-only authenticated supervisor peer that identifies the current build as
its live direct child. Host characteristics and artifacts may support
observability but do not establish ownership.

## Consequences

- **Positive:** Update approval cannot silently select a backend merely because
  it is conventional for the host.
- **Positive:** Deployment diagnostics distinguish development, unmanaged,
  ambiguous, and unsupported runtimes instead of presenting false certainty.
- **Negative:** Temporary ownership probes can disable update approval until
  evidence is available again.
- **Neutral:** Browser locality, privileges, tools, and deployment claims remain
  separate authority decisions.

## References

- ADR-017: production deployment keeps backend-owned activation
- ADR-018: release updates use approval-bound installations
- `api::installation_ownership`
- `specs/deployment-info/requirements.md`
- `specs/release-updates/requirements.md`
