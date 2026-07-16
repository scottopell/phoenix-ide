# Cross-Platform Production Deployment — Executive Status

## Current reality

The shared contract covers launchd, systemd, and bare Linux with common candidate preparation and backend-owned activation. Native macOS launchd uses a distinct one-shot LaunchAgent. Linux systemd uses a validated root-owned transaction, a transient activation unit, atomic installation, exact identity verification, durable status and claim fencing, and verified rollback. The bare-Linux path still uses detached-process behavior and does not satisfy the shared activation contract.

The Lima/VZ harness proves successful systemd activation and exact-identity rollback with real socket/service units, changed `MainPID`, truthful `deployed.sha`, terminal claim release, and survival after termination of the initiating SSH process group. Broader failure injection and reboot persistence coverage remain outstanding. The bare-Linux supervisor is not implemented.

Live production deployment remains an explicitly gated operator action; automated integration validation uses disposable resources.

## Requirement coverage

| Requirement | Current implementation / verification |
| --- | --- |
| REQ-PD-001 | `detect_prod_env`; backend status naming requires normalization |
| REQ-PD-002 | typed local/release candidate preparation shared by launchd and systemd; bare-Linux release wiring not implemented |
| REQ-PD-003 | launchd and systemd complete preparation paths and tests; bare preparation not implemented |
| REQ-PD-004 | launchd one-shot helper and systemd root transient unit; persistent bare owner not implemented |
| REQ-PD-005 | launchd and systemd immutable handoffs with secret-redaction tests; bare handoff not implemented |
| REQ-PD-006 | launchd and systemd claim/status fencing tests; bare claims not implemented |
| REQ-PD-007 | systemd root staging validates fixed targets, ownership, modes, hashes, units, users, and symlink safety |
| REQ-PD-008 | launchd and systemd destination-filesystem reservation and atomic replacement; bare installation not implemented |
| REQ-PD-009 | launchd and systemd exact version/SHA verification with changed backend-owned process; bare child binding not implemented |
| REQ-PD-010 | launchd and systemd verified rollback; bare rollback not implemented |
| REQ-PD-011 | launchd and systemd durable terminal status, truthful SHA, and fenced claim release; bare status not implemented |
| REQ-PD-012 | `.phoenix-ide.env` exists, but launchd override JSON and systemd drop-ins remain competing stores |
| REQ-PD-013 | local/release deploy and durable status exist for launchd/systemd; bare release deployment is unavailable; `prod set`/`prod unset` still mutate backend override stores instead of returning `.phoenix-ide.env` guidance |
| REQ-PD-014 | persistent bare-Linux supervisor not implemented |
| REQ-PD-015 | bare-Linux reboot persistence not implemented |
| REQ-PD-016 | launchd disposable harness and Lima/VZ systemd success/rollback harness; extended systemd failure/reboot matrix and Docker bare-Linux harness not implemented |
| REQ-PD-017 | Linux x86_64 asset exists; Linux aarch64 publication and cross-platform selection verification not implemented |

## Operator surface

The target surface for each backend is:

- `./dev.py prod deploy` — checked local `HEAD` build.
- `./dev.py prod deploy --release vX.Y.Z` — exact published release.
- `./dev.py prod deploy --release latest` — latest stable release resolved once.
- `./dev.py prod status` — selected backend, runtime identity, and durable transaction result.
- `./dev.py prod stop` — backend-owned runtime stop.
- `./dev.py prod set` / `prod unset` — rejected with guidance to edit `.phoenix-ide.env` directly.
