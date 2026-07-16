# Cross-Platform Production Deployment — Executive Status

## Current reality

The shared contract covers launchd, systemd, and bare Linux with common candidate preparation and backend-owned activation. Native macOS launchd satisfies the transactional activation contract through a distinct one-shot LaunchAgent. The systemd and bare-Linux paths still use direct service replacement and detached-process behavior and therefore do not yet satisfy the shared activation, rollback, claim, release-deployment, or configuration requirements.

A Lima/VZ qualification harness proves that an Ubuntu 24.04 ARM64 systemd transient root unit survives termination of the initiating SSH process group. The production systemd transaction helper and bare-Linux supervisor are not implemented.

Live production deployment remains an explicitly gated operator action; automated integration validation uses disposable resources.

## Requirement coverage

| Requirement | Current implementation / verification |
| --- | --- |
| REQ-PD-001 | `detect_prod_env`; backend status naming requires normalization |
| REQ-PD-002 | launchd `launchd_prod_deploy`, `_prepare_release_candidate`, `prod_build`; Linux release preparation not implemented |
| REQ-PD-003 | launchd preparation and tests; systemd/bare preparation not implemented |
| REQ-PD-004 | launchd one-shot helper; Lima transient-unit ownership qualification; production systemd/bare owners not implemented |
| REQ-PD-005 | launchd manifest and secret-redaction tests; Linux handoffs not implemented |
| REQ-PD-006 | launchd claim/status tests; systemd/bare claims not implemented |
| REQ-PD-007 | systemd privileged handoff not implemented |
| REQ-PD-008 | launchd `atomic_install`; systemd/bare atomic installation not implemented |
| REQ-PD-009 | launchd `wait_for_identity`; systemd/bare exact process-bound verification not implemented |
| REQ-PD-010 | launchd `restore` and rollback tests; systemd/bare verified rollback not implemented |
| REQ-PD-011 | launchd durable status and stale-status tests; systemd/bare durable transaction status not implemented |
| REQ-PD-012 | `.phoenix-ide.env` exists, but launchd override JSON and systemd drop-ins remain competing stores |
| REQ-PD-013 | local deploy/status/stop exist on all backends; release deploy is launchd-only; `prod set`/`prod unset` still mutate backend override stores instead of returning `.phoenix-ide.env` guidance |
| REQ-PD-014 | persistent bare-Linux supervisor not implemented |
| REQ-PD-015 | bare-Linux reboot persistence not implemented |
| REQ-PD-016 | launchd disposable harness and Lima/VZ systemd qualification harness; Docker bare-Linux harness not implemented |
| REQ-PD-017 | Linux x86_64 asset exists; Linux aarch64 publication and cross-platform selection verification not implemented |

## Operator surface

The target surface for each backend is:

- `./dev.py prod deploy` — checked local `HEAD` build.
- `./dev.py prod deploy --release vX.Y.Z` — exact published release.
- `./dev.py prod deploy --release latest` — latest stable release resolved once.
- `./dev.py prod status` — selected backend, runtime identity, and durable transaction result.
- `./dev.py prod stop` — backend-owned runtime stop.
- `./dev.py prod set` / `prod unset` — rejected with guidance to edit `.phoenix-ide.env` directly.
