# Native macOS launchd Deployment — Executive Status

## Current reality

Native macOS production deployment prepares either local `HEAD` or a checksummed published release, stages rollback inputs and a redacted manifest, and transfers activation to a distinct one-shot LaunchAgent. The helper serializes activation, performs atomic replacement, requires exact `/api/version` identity, and attempts verified rollback on failure. Durable status is reported by `./dev.py prod status`.

Installed-runtime restart is a separate launchd-owned transaction. It sends SIGHUP without unloading the socket-activated target, verifies a new PID with the same exact identity, preserves the binary, plist, environment, listener, and deployed SHA, and reports its own durable status without replacing deployment status.

Live production deployment remains an explicitly gated operator action; automated validation uses disposable resources.

## Requirement coverage

| Requirement | Implementation / verification |
| --- | --- |
| REQ-LDD-001 | `_helper_plist`, `launchd_prod_deploy`; disposable integration harness |
| REQ-LDD-002 | `launchd_prod_deploy`, `_binary_identity`; preparation tests |
| REQ-LDD-003 | `_claim_launchd_deploy`, helper `flock`; concurrent-deploy tests |
| REQ-LDD-004 | helper `Manifest`; secret-safe metadata test |
| REQ-LDD-005 | `atomic_install`; atomic install test |
| REQ-LDD-006 | `Launchctl.stop`, `Launchctl.start`; transition tests |
| REQ-LDD-007 | `wait_for_identity`; wrong-identity rollback test |
| REQ-LDD-008 | `restore`, `activate`; rollback outcome tests |
| REQ-LDD-009 | `write_status`, post-verification `deployed.sha`; success test |
| REQ-LDD-010 | `launchd_prod_status`; stale-status test |
| REQ-LDD-011 | `_prepare_release_candidate`, `prod_build`; release preparation tests |
| REQ-LDD-012 | `main` prod parser; positional rejection test |
| REQ-LDD-013 | `tests/integration/launchd_deploy_harness.py`; macOS-gated harness |
| REQ-LDD-014 | `launchd_prod_restart`, `launchd_restart_helper.restart`; unit tests and disposable launchd restart journey |
| REQ-LDD-015 | `_claim_launchd_restart`, `/api/version` socket-activation report, restart helper LaunchAgent handoff; runtime-activation, mutual-exclusion, and secret-redaction tests |
| REQ-LDD-016 | `launchd_restart_helper.wait_for_identity`, restart status; exact-identity and failure-state tests |

## Operator surfaces

- `./dev.py prod deploy` — checked local `HEAD` build.
- `./dev.py prod deploy --release vX.Y.Z` — exact published release.
- `./dev.py prod deploy --release latest` — latest stable release resolved once.
- `./dev.py prod status` — launchd PID/runtime identity and durable transaction result.
- `./dev.py prod restart` — restart the installed process in place without rebuilding or changing installed configuration.
