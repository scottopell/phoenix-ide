# Cross-Platform Production Deployment — Executive Status

## Current reality

The shared contract covers launchd, systemd, and bare Linux with common candidate preparation and backend-owned activation. Native macOS launchd uses a distinct one-shot LaunchAgent. Linux systemd uses a validated root-owned transaction and transient activation unit. Bare Linux uses a persistent same-user supervisor with an owner-only Unix socket and direct Phoenix child ownership. All three backends provide immutable handoff, atomic installation, exact identity verification, durable status and claim fencing, and verified rollback.

The Lima/VZ harness proves successful systemd activation and exact-identity rollback with real socket/service units, changed `MainPID`, truthful `deployed.sha`, terminal claim release, and survival after termination of the initiating SSH process group. It also verifies the bare-Linux transaction engine's direct child ownership, `/proc` start-time binding, exact identity, verified rollback, and child-only stop. Bare supervisor restart reconciliation, production-style detached-start acceptance, reboot persistence, and broader systemd failure and reboot coverage remain outstanding.

Live production deployment remains an explicitly gated operator action; automated integration validation uses disposable resources.

## Requirement coverage

| Requirement | Current implementation / verification |
| --- | --- |
| REQ-PD-001 | `detect_prod_env`; backend status naming requires normalization |
| REQ-PD-002 | typed local/release candidate preparation shared by launchd, systemd, and bare Linux |
| REQ-PD-003 | all three backends have complete local/release preparation paths and focused tests |
| REQ-PD-004 | launchd one-shot helper, systemd root transient unit, and persistent same-user bare supervisor own activation |
| REQ-PD-005 | all three backends use immutable handoffs; launchd/systemd include secret-redaction coverage and bare IPC accepts only transaction identity and manifest hash |
| REQ-PD-006 | all three backends implement durable claim/status fencing and terminal claim release |
| REQ-PD-007 | systemd root staging validates fixed targets, ownership, modes, hashes, units, users, and symlink safety |
| REQ-PD-008 | all three backends reserve destination-filesystem rollback capacity and atomically replace installation artifacts |
| REQ-PD-009 | all three backends verify exact version/SHA and backend-owned process identity; bare Linux binds its direct child by PID and `/proc` start time |
| REQ-PD-010 | all three backends implement exact verified rollback |
| REQ-PD-011 | all three backends persist durable terminal status and truthful SHA with fenced claim release |
| REQ-PD-012 | `.phoenix-ide.env` exists, but launchd override JSON and systemd drop-ins remain competing stores |
| REQ-PD-013 | local/release deploy, durable status, and stop exist for all three backends; `prod set`/`prod unset` still mutate backend override stores instead of returning `.phoenix-ide.env` guidance |
| REQ-PD-014 | persistent same-user bare-Linux supervisor owns the direct Phoenix child; restart reconciliation and detached-start initiator-death acceptance remain outstanding |
| REQ-PD-015 | bare-Linux reboot persistence not implemented |
| REQ-PD-016 | launchd disposable harness and Lima/VZ systemd success/rollback plus bare supervisor-core acceptance; extended systemd failure/reboot coverage and production-style detached bare-supervisor acceptance remain outstanding |
| REQ-PD-017 | Linux x86_64 asset exists; Linux aarch64 publication and cross-platform selection verification not implemented |

## Operator surface

The target surface for each backend is:

- `./dev.py prod deploy` — checked local `HEAD` build.
- `./dev.py prod deploy --release vX.Y.Z` — exact published release.
- `./dev.py prod deploy --release latest` — latest stable release resolved once.
- `./dev.py prod status` — selected backend, runtime identity, and durable transaction result.
- `./dev.py prod stop` — backend-owned runtime stop.
- `./dev.py prod set` / `prod unset` — rejected with guidance to edit `.phoenix-ide.env` directly.
