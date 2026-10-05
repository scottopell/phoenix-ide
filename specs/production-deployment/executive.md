# Cross-Platform Production Deployment — Executive Status

## Current reality

The shared contract covers launchd, systemd, and bare Linux with common candidate preparation and backend-owned activation. Native macOS launchd uses a distinct one-shot LaunchAgent. Linux systemd uses a validated root-owned transaction and transient activation unit. Bare Linux uses a persistent same-user supervisor with an owner-only Unix socket and direct Phoenix child ownership. All three backends require a modern candidate's complete 40-character embedded SHA to equal its selected source commit, provide immutable handoff, atomic installation, durable status and claim fencing, and verify runtime-artifact rollback. A captured already-installed rollback predecessor may retain a legacy 12-character identity, but that acceptance does not extend to candidates, controllers, or general downgrade compatibility.

The Lima/VZ harness proves successful systemd activation and exact-identity rollback with real socket/service units, changed `MainPID`, truthful `deployed.sha`, terminal claim release, and survival after termination of the initiating SSH process group. It also verifies the bare-Linux transaction engine's direct child ownership, `/proc` start-time binding, exact identity, verified rollback, and child-only stop. Bare supervisor startup reconciles interrupted durable phases and re-establishes exact direct-child ownership after restart. Production-style detached-start acceptance verifies survival after launcher exit, socket-only commit and rollback, and child-only stop. Installation configures owner `@reboot` cron when available and otherwise emits exact same-user host rc guidance without claiming persistence. Systemd acceptance verifies committed runtime recovery across VM reboot with changed `MainPID`, exact identity, and unchanged durable status. Transaction journeys use the deterministic fixture runtime; a separately built aarch64 musl Phoenix binary has also been smoke-tested in disposable Lima with exact `--build-identity` and `/api/version` verification.

Ordinary rollback restores runtime artifacts and configuration without a database compatibility guarantee. The explicit prepared-artifact launchd ProductConversation upgrade is a narrow exception: a verified offline SQLite snapshot, binary, and configuration are restored together before predecessor startup; failed recovery retains its ownership fence and attempts teardown. On bare Linux, a running supervisor is also a persistent activation authority rather than a protocol-compatible replaceable controller: deployment fails before disruption when its installed artifact differs from the selected supervisor artifact, even if protocol versions match.

Live production deployment remains an explicitly gated operator action; automated integration validation uses disposable resources.

The paired launchd controller privately bootstraps candidate/predecessor plists
and delays auto-load publication until their required verification boundary.
Pre-snapshot failures resume only an unchanged, exclusively owned legacy pair;
post-commit warnings do not authorize rollback or establish reboot persistence.
The lifecycle/OS/filesystem/claim boundary table is in
`specs/launchd-deployment/executive.md`. These refinements are qualification-only,
not a later live deployment on devmbp.

## Main implementation gap

The accepted paired contract (REQ-PD-018, REQ-PD-019 and REQ-PD-020) and behavioral model are normative,
but their controller implementation is not yet shipped on `main`. PR #836 owns
prepared receipt admission, private paired SQLite recovery, claim/status behavior,
and regression qualification. The documentation prerequisite PR #842 changes no
runtime. Until #836 lands, main does not implement these paired guarantees;
ordinary deployment/restart behavior is separate. Local/source evidence below
must not be read as main coverage or live verification of every refinement.

The committed finalization-only resumer is commissioned by Global as delegated
engineering scope, not a new quoted user release/deployment authorization.
It is qualified locally/CI only; no healthy production finalization was executed.

## Requirement coverage

| Requirement | Current implementation / verification |
| --- | --- |
| REQ-PD-001 | `detect_prod_env`; backend status naming requires normalization |
| REQ-PD-002 | typed local/release candidate preparation requires the complete 40-character embedded SHA to equal the selected full source commit across launchd, systemd, and bare Linux |
| REQ-PD-003 | all three backends have complete local/release preparation paths and focused tests |
| REQ-PD-004 | launchd one-shot helper, systemd root transient unit, and persistent same-user bare supervisor own activation |
| REQ-PD-005 | all three backends use immutable handoffs; launchd/systemd include secret-redaction coverage and bare IPC accepts only transaction identity and manifest hash |
| REQ-PD-006 | all three backends implement durable claim/status fencing and terminal claim release |
| REQ-PD-007 | systemd root staging validates fixed targets, ownership, modes, hashes, units, users, and symlink safety |
| REQ-PD-008 | all three backends reserve destination-filesystem rollback capacity and atomically replace installation artifacts |
| REQ-PD-009 | all three backends verify exact version and full-SHA equality with the selected commit; bare Linux also binds its direct child by PID and `/proc` start time |
| REQ-PD-010 | all three backends verify runtime-artifact rollback; predecessor validation accepts only captured 12- or 40-character lowercase identities and makes no database recovery or general downgrade guarantee |
| REQ-PD-011 | all three backends persist durable terminal status and truthful SHA with fenced claim release |
| REQ-PD-012 | modern deployment snapshots only `.phoenix-ide.env`; legacy launchd JSON and systemd drop-ins are neither consulted nor migrated |
| REQ-PD-013 | local/release deploy, durable status, and stop exist for all three backends; `prod set`/`prod unset` reject without mutation and direct operators to `.phoenix-ide.env` |
| REQ-PD-014 | persistent same-user bare-Linux supervisor owns the direct Phoenix child and reconciles interrupted durable phases; deployment refuses a changed running supervisor artifact before disruption rather than treating protocol equality as controller compatibility |
| REQ-PD-015 | bare installation starts independently for the active boot, installs an idempotent owner `@reboot` entry when compatible crontab is available, and otherwise prints exact same-user host rc guidance without claiming persistence |
| REQ-PD-016 | launchd disposable harness; Lima/VZ systemd success, rollback, initiator-death, and committed reboot recovery; detached bare-supervisor commit/rollback/stop acceptance; disposable aarch64 musl Phoenix build-identity and version-endpoint smoke |
| REQ-PD-017 | release workflow builds native macOS and musl Linux assets for x86_64 and aarch64, refuses incomplete asset sets before checksumming, and deployment selection tests cover all four targets |
| REQ-PD-018 | explicit prepared-artifact launchd path verifies exact candidate identity separately from clean controller/helper binding; retains private matched database/runtime proof and fails closed on incomplete recovery |
| REQ-PD-019 | prepared path preserves installed environment and PATH without model/default changes |

## Operator surface

The target surface for each backend is:

- `./dev.py prod deploy` — checked local `HEAD` build.
- `./dev.py prod deploy --release vX.Y.Z` — exact published release.
- `./dev.py prod deploy --release latest` — latest stable release resolved once.
- `./dev.py prod deploy --prepared-artifact DIR --expected-full-commit SHA --paired-database-upgrade` — explicit legacy launchd ProductConversation upgrade from protected prepared bytes. All options are required; the directory is trusted operator input whose workflow/source association must be established independently of local receipt validation.
- `./dev.py prod status` — selected backend, runtime identity, and durable transaction result.
- `./dev.py prod stop` — backend-owned runtime stop.
- `./dev.py prod set` / `prod unset` — rejected with guidance to edit `.phoenix-ide.env` directly.

Backend-specific restart surfaces preserve their owning runtime contract: launchd restarts the installed process without changing installed state, while bare Linux re-snapshots `.phoenix-ide.env` through the supervisor transaction. Systemd does not expose `prod restart`.
