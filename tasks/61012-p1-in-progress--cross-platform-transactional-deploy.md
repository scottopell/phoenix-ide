# Complete cross-platform transactional production deployment

Extend the transactional production deployment foundation from native macOS launchd to Linux systemd and bare Linux without systemd. Support checked local HEAD and immutable published-release candidates on all three backends, with independently owned activation, exact runtime verification, durable status, atomic replacement, and verified rollback.

## Locked scope

- `.phoenix-ide.env` is the modern configuration source of truth.
- Do not detect legacy deployment artifacts or Phoenix ancestors.
- Do not refuse legacy in-Phoenix deployment automatically.
- Do not add `deploy-init`, automatically migrate legacy configuration, or convert legacy deployment mechanisms.
- Preserve normal Phoenix process cleanup; safety comes from external activation ownership.
- Do not run live production deploy, stop, restart, rollback, or failure injection without separate immediate approval.

## Required design work

- Inventory current systemd and detached-daemon paths and shared launchd preparation.
- Specify shared candidate preparation and backend-specific activation ownership.
- Design a root-owned transient systemd activation helper with validated immutable staged inputs.
- Design the smallest secure same-user persistent supervisor for bare Linux, including its practical autostart contract.
- Generalize requirements, Allium lifecycle, executive coverage, release assets, status, and tests without rewriting historical ADR-010.
- Add detailed legacy migration completion notes requested in task 61011.

## Implementation plan

### 1. Generalize the contract before behavior

- Add a cross-platform production-deployment spEARS/Allium contract with a backend discriminator for launchd, systemd, and bare Linux.
- Preserve ADR-010 as launchd history and add a new ADR for shared preparation plus backend-owned activation.
- Keep backend-specific ownership and transition rules explicit; do not hide them in untyped metadata.

### 2. Extract shared preparation without changing launchd semantics

- Introduce typed standard-library Python values for source identity, runtime identity, artifact hashes, endpoints, claims, status, and backend kind.
- Refactor local-HEAD and published-release selection to return one verified candidate type.
- Parameterize release assets by host target; support x86_64 and aarch64 Linux musl assets where CI can produce them reliably.
- Preserve the launchd helper protocol and tests while moving only genuinely shared logic.

### 3. Implement transactional systemd activation

- Stage candidate binary, generated service/socket units, env snapshot, rollback artifacts, helper, and immutable manifest before disruption.
- Copy immutable inputs into a root-owned transaction directory and launch the helper through `sudo systemd-run`; the transient unit must outlive Phoenix and the initiating shell.
- The root helper validates allowed target paths, ownership, modes, protocol, and hashes; atomically installs artifacts; reloads systemd; observes service/socket/PID state; verifies exact `/api/version`; and commits or performs verified rollback.
- Durable status must remain readable by the deploying user without exposing env values. Claim release requires a durable terminal state.
- Remove modern systemd drop-in configuration precedence; `.phoenix-ide.env` is snapshotted once and installed as the complete modern service environment.

### 4. Replace the detached daemon with a bare-Linux supervisor

- Install a standard-library-only supervisor and managed binary/config under `~/.phoenix-ide`, all owner-only where secret-bearing.
- The supervisor owns Phoenix as its direct child and accepts one-shot immutable transaction references over an owner-only Unix socket. On Linux it verifies the peer UID with `SO_PEERCRED`; transaction IDs and manifest hashes prevent stale handoff confusion.
- The supervisor reserves candidate and rollback install space before stopping its child, atomically activates, verifies PID/start-time plus exact runtime identity, and performs verified rollback.
- `prod deploy`, `prod status`, and `prod stop` ensure the supervisor is running independently of Phoenix. Installation attempts owner crontab `@reboot` persistence only when a compatible crontab is available; otherwise it reports that reboot persistence requires the host's rc mechanism. It must not claim universal reboot autostart without an init facility.
- On supervisor restart, nonterminal durable state is reconciled conservatively: verify the active child/artifacts or restore the last verified runtime; never infer success from a PID file alone.

### 5. Unify operator surfaces and remove modern override stores

- Support `prod deploy`, `prod deploy --release TAG`, and `prod deploy --release latest` on all three backends.
- Make status report backend runtime identity plus the shared durable transaction result.
- Make `prod set`/`prod unset` edit `.phoenix-ide.env` atomically or remove those commands with direct migration guidance; do not retain separate launchd/systemd override stores as modern sources of truth.
- Keep stop backend-owned and transaction-aware.

### 6. Prove each ownership boundary with disposable tests

- Add shared preparation and manifest-validation tests for local and release candidates on every backend.
- Add a disposable systemd harness, gated to hosts with systemd and sudo test capability, that uses unique units, temporary paths, an isolated DB, and an ephemeral port and structurally refuses production resources.
- Add a bare-supervisor harness that kills the initiating process group, proves supervisor/child ownership, exercises successful activation and failed activation rollback, and tests restart reconciliation.
- Test ENOSPC before disruption, concurrent rejection, claim/status durability, exact identity mismatch, config rollback, secret redaction, helper protocol mismatch, and both Linux architectures' release selection.
- Never exercise live production labels, paths, units, database, or port without separate immediate approval.

## Completion documentation

Before marking this task done, append detailed legacy migration notes to task 61011 covering the old launchd, systemd, and detached-daemon artifacts and behavior; manual external-terminal/tmux migration; preservation of database/data paths; reconstruction from `.phoenix-ide.env`; post-migration exact verification; rollback/recovery; and implementation hints for future detection/automation. State explicitly that Phoenix does not detect or prevent unsafe legacy self-deploy and does not automatically migrate legacy artifacts.
