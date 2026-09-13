# ADR-050: launchd restart preserves installed state through an independent helper

- **Status:** Accepted
- **Date:** 2026-09-13
- **Affects:** REQ-LDD-014 through REQ-LDD-016; `RestartTransaction`

## Context

Phoenix's macOS LaunchAgent already owns the production listener and keeps it open while SIGHUP causes the process to exit and launchd starts a replacement. The production command surface nevertheless rejects restart on macOS, forcing an operator to run the materially broader deployment lifecycle even when the installed binary and configuration must remain unchanged.

A restart can be initiated from Phoenix's own terminal. The initiating process and connection therefore cannot own post-signal verification. Restart also must not race the deployment helper, silently adopt checkout configuration, or imply that rollback occurred when no installation artifact was replaced.

## Options considered

1. **Treat restart as deployment of the installed binary** — reuses the deployment helper, but replaces the plist and binary, unloads the target job, and conflates an identity-preserving process restart with artifact installation and rollback.
2. **Signal the target directly from `dev.py` and return** — preserves installed artifacts, but loses independent verification when Phoenix or the invoking terminal disappears and provides no durable result.
3. **Use a distinct one-shot LaunchAgent with shared operation fencing** — preserves the loaded socket and installed state while giving launchd ownership of signaling, exact verification, and durable status independently of Phoenix.

## Decision

Use a distinct one-shot LaunchAgent for macOS production restart. Preparation validates a modern socket-activated installation, binds the running PID and exact runtime identity to hashes of the installed binary, plist, and deployed source marker, and hands a secret-free immutable manifest to the helper. The helper shares the deployment activation lock and a mutually exclusive claim boundary, revalidates the installed state, sends SIGHUP through launchctl without unloading the target job, and commits only after observing a new PID serving the same exact identity with unchanged artifact hashes.

Restart has its own durable status authority so it cannot overwrite the deployment transaction used by published-release recovery. It preserves the installed plist environment and does not read `.phoenix-ide.env`; configuration changes remain deployments. Failure after signaling is reported as restart failure, not rollback, because the operation installs nothing to restore.

## Consequences

- **Positive:** Operators can restart Phoenix without compilation, repository checks, configuration drift, or installation replacement.
- **Positive:** The listener remains launchd-owned while the process changes, and verification survives loss of the initiating Phoenix connection.
- **Positive:** Release-update hydration continues to read the unmodified deployment status authority.
- **Negative:** macOS maintains a second small helper protocol and restart-status lifecycle alongside deployment.
- **Negative:** Restart refuses legacy or partially managed installations that cannot prove exact identity, socket ownership, KeepAlive behavior, and deployed source identity; deployment is required to modernize them.
- **Neutral:** Existing in-process connections close with the old process even though the listening socket remains available for queued or new connections.

## References

- ADR-010
- ADR-017
- `specs/launchd-deployment/requirements.md`
- `specs/launchd-deployment/launchd-deployment.allium`
- `launchd_prod_restart`
- `scripts/launchd_restart_helper.py`
