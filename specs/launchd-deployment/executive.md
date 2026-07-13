# Native macOS launchd Deployment — Executive Status

## Current reality

Native macOS production deployment prepares either local `HEAD` or a checksummed published release, stages rollback inputs and a redacted manifest, and transfers activation to a distinct one-shot LaunchAgent. The helper serializes activation, performs atomic replacement, requires exact `/api/version` identity, and attempts verified rollback on failure. Durable status is reported by `./dev.py prod status`.

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

## Operator surfaces

- `./dev.py prod deploy` — checked local `HEAD` build.
- `./dev.py prod deploy --release vX.Y.Z` — exact published release.
- `./dev.py prod deploy --release latest` — latest stable release resolved once.
- `./dev.py prod status` — launchd PID/runtime identity and durable transaction result.
