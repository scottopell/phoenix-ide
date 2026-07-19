# ADR-018: Release updates use published release previews and approval-bound installations

- **Status:** Accepted
- **Date:** 2026-07-16
- **Affects:** REQ-RU-001 through REQ-RU-010

## Context

Phoenix needs an in-app release-update surface that tells an operator which stable published version is available, what would change if they install it, and whether an installation is still in progress or has finished after the page reconnects. The update surface must reuse the production-deployment contract for release identity and exact verification without turning the UI into a second deployment engine.

The user must approve an exact published artifact before Phoenix replaces itself. That approval is meaningful only when it binds the release tag to the full embedded commit and to the preview the operator saw. The controller that shows release availability and the backend that performs installation must be able to progress independently because Phoenix may restart or disconnect during activation.

A release update also needs truthful operator feedback. If a terminal promises success, the installed runtime identity must have been verified. If rollback happened, the surface must say so explicitly rather than implying that the new release stuck. Meanwhile `./dev.py` remains a bootstrap and local-development tool; online published-release discovery belongs to the application runtime, not to an offline developer workflow.

## Options considered

1. **Treat release updates as a thin UI wrapper around the existing production commands** — reuses backend logic, but the preview can drift from the eventual installation target, reconnect loses context, and approval is not structurally bound to the exact published commit the user reviewed.
2. **Let the UI own release discovery, preview, and installation state directly** — keeps the browser responsive, but duplicates release identity, durable status, and rollback truth outside the backend-owned deployment contract.
3. **Use backend-owned published-release previews plus explicit approval bound to tag and full commit, with controller/backend lifecycles kept independent** — preserves one release-identity authority, allows reconnect hydration from durable backend status, and keeps user-visible installation outcomes truthful.

## Decision

Use backend-owned stable published release discovery and immutable preview records as the source for the in-app update surface. A preview resolves one stable published release to one tag, one full commit, one host-target asset, and one human-readable summary of what will be installed. The user's approval binds to that exact preview identity; a later installation attempt must either install that exact tag+commit pair or require a fresh preview and fresh approval.

Keep the release-update controller and the installation backend as distinct lifecycles. The controller may be created, disconnected, reconnected, or dismissed without changing the backend transaction. The backend may continue through preparation, handoff, verification, rollback, and terminal status while Phoenix restarts. On reconnect, the controller hydrates from durable backend status rather than inferring success from a disconnected browser state.

The update surface adopts the production-deployment contract rather than redefining it. Published release identity, immutable handoff, backend-owned activation, exact runtime verification, durable terminal status, and verified rollback remain owned by `specs/production-deployment/`. This ADR adds the operator-facing binding rules that sit above that contract: stable-only discovery, immutable preview, explicit same-host approval, reconnect hydration, and truthful terminal language. `./dev.py` stays outside this capability and remains an offline/bootstrap workflow.

## Consequences

- **Positive:** the operator sees one stable published candidate and approves the exact artifact Phoenix will attempt to install.
- **Positive:** reconnect or Phoenix restart does not erase update progress because the controller can rehydrate from durable backend facts.
- **Positive:** success and rollback messages stay aligned with exact runtime verification instead of optimistic UI inference.
- **Negative:** the system must persist preview identity and approval binding separately from transient UI state.
- **Negative:** same-host approval adds a locality gate that remote browsers cannot bypass, so some users need a same-host session to approve installation.
- **Neutral:** release notes and preview presentation remain a consumer of published release metadata, not a new release-authoring system.

## References

- ADR-017
- `specs/production-deployment/requirements.md`
- `specs/production-deployment/production-deployment.allium`
- `specs/release-updates/requirements.md`
- `specs/release-updates/release-updates.allium`
