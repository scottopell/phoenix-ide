# In-App Release Updates — Executive Status

## Current reality

Phoenix exposes a typed in-app published-release update surface on the About deployment page. It discovers GitHub's latest stable release, resolves the tag to a full source commit, selects the host asset and published checksum, and presents that immutable identity and release notes before any approval is possible.

Approval is same-host only and binds the exact preview tag plus full commit. The server downloads that commit's deployment controller into an owner-only transaction artifact and waits until the controller's transaction ID appears in backend-owned durable status. Launchd, systemd, and bare Linux continue to own disruption, exact runtime verification, commit, and rollback through the production-deployment stack; the running Phoenix process never replaces or restarts itself.

After reconnect, the UI hydrates the authoritative native status file. In-progress, committed, precondition-failed, verified rollback, rollback failure, concurrent rejection, unreadable, and stale outcomes remain distinct. `./dev.py` remains available for bootstrap, local HEAD deployment, offline repair, migration, and emergency recovery.

## Requirement coverage

| Requirement | Implementation / verification |
| --- | --- |
| REQ-RU-001 | `release_updates::discover_release` accepts GitHub's stable latest release and rejects prereleases. |
| REQ-RU-002 | `ReleasePreview` and `ReleaseUpdatePanel` present tag, full commit, asset, checksum, notes, and running identity before approval. |
| REQ-RU-003 | `client_is_local`, `valid_approval`, and controller exact-tag/full-commit validation enforce same-host, preview-bound approval. |
| REQ-RU-004 | The pinned `dev.py` controller delegates to the existing launchd/systemd/bare activation owners. |
| REQ-RU-005 | `release_updates::approve` launches the controller independently and returns only after durable backend handoff. |
| REQ-RU-006 | `read_status` normalizes backend-owned status and the UI polls/restores it after reconnect. |
| REQ-RU-007 | Committed status displays backend-verified expected identity and approved source commit. |
| REQ-RU-008 | `TransactionStatus` renders preparation failure, verified rollback, rollback failure, rejection, unreadable, and stale recovery distinctly. |
| REQ-RU-009 | Native status supplies transaction ID, approved source, expected runtime identity, timestamps, and failure evidence without a parallel SQLite copy. |
| REQ-RU-010 | Normal `./dev.py prod deploy` behavior remains available; controller mode is additive and explicit. |

## Verification

- `allium check specs/release-updates/release-updates.allium`
- controller-mode tests across launchd, systemd, and bare Linux in `tests/devpy/`
- Rust release identity and approval-binding tests in `api::release_updates::tests`
- `ReleaseUpdatePanel.test.tsx` covers immutable preview, explicit approval, remote denial, unreadable status, verified rollback, and rollback failure
- `./dev.py check` validates all repository lanes, including musl cross-compilation and generated TypeScript freshness
