# Cross-Platform Production Deployment — Executive Status

## Current reality

The shared contract covers launchd, systemd, and bare Linux with common candidate preparation and backend-owned activation. Native macOS launchd uses a distinct one-shot LaunchAgent. Linux systemd uses a validated root-owned transaction and transient activation unit. Bare Linux uses a persistent same-user supervisor with an owner-only Unix socket and direct Phoenix child ownership. All three backends provide immutable handoff, atomic installation, exact identity verification, durable status and claim fencing, and verified rollback.

The Lima/VZ harness proves successful systemd activation and exact-identity rollback with real socket/service units, changed `MainPID`, truthful `deployed.sha`, terminal claim release, and survival after termination of the initiating SSH process group. It also verifies the bare-Linux transaction engine's direct child ownership, `/proc` start-time binding, exact identity, verified rollback, and child-only stop. Bare supervisor startup reconciles interrupted durable phases and re-establishes exact direct-child ownership after restart. Production-style detached-start acceptance verifies survival after launcher exit, socket-only commit and rollback, and child-only stop. Installation configures owner `@reboot` cron when available and otherwise emits exact same-user host rc guidance without claiming persistence. Systemd acceptance verifies committed runtime recovery across VM reboot with changed `MainPID`, exact identity, and unchanged durable status. Transaction journeys use the deterministic fixture runtime; a separately built aarch64 musl Phoenix binary has also been smoke-tested in disposable Lima with exact `--build-identity` and `/api/version` verification.

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
| REQ-PD-012 | modern deployment snapshots only `.phoenix-ide.env`; legacy launchd JSON and systemd drop-ins are neither consulted nor migrated |
| REQ-PD-013 | local/release deploy, durable status, and stop exist for all three backends; `prod set`/`prod unset` reject without mutation and direct operators to `.phoenix-ide.env` |
| REQ-PD-014 | persistent same-user bare-Linux supervisor owns the direct Phoenix child, reconciles interrupted durable phases, and has production-style detached initiator-death acceptance |
| REQ-PD-015 | bare installation starts independently for the active boot, installs an idempotent owner `@reboot` entry when compatible crontab is available, and otherwise prints exact same-user host rc guidance without claiming persistence |
| REQ-PD-016 | launchd disposable harness; Lima/VZ systemd success, rollback, initiator-death, and committed reboot recovery; detached bare-supervisor commit/rollback/stop acceptance; disposable aarch64 musl Phoenix build-identity and version-endpoint smoke |
| REQ-PD-017 | release workflow builds native macOS and musl Linux assets for x86_64 and aarch64, refuses incomplete asset sets before checksumming, and deployment selection tests cover all four targets |

## Operator surface

The target surface for each backend is:

- `./dev.py prod deploy` — checked local `HEAD` build.
- `./dev.py prod deploy --release vX.Y.Z` — exact published release.
- `./dev.py prod deploy --release latest` — latest stable release resolved once.
- `./dev.py prod status` — selected backend, runtime identity, and durable transaction result.
- `./dev.py prod stop` — backend-owned runtime stop.
- `./dev.py prod set` / `prod unset` — rejected with guidance to edit `.phoenix-ide.env` directly.
