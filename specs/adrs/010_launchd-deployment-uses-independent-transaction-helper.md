# ADR-010: launchd deployment uses an independent transaction helper

- **Status:** Accepted
- **Date:** 2026-07-13
- **Affects:** REQ-LDD-001 through REQ-LDD-010

## Context

Phoenix may initiate replacement of its own macOS LaunchAgent. Stopping that agent terminates direct PTYs, WebSockets, and Phoenix-owned agent process groups, so an in-process deploy cannot be responsible for restoring the service. The replacement also spans several fallible steps while preserving a known-good binary and secret-bearing plist.

## Options considered

1. **Continue activation in the initiating process** — simple, but the service shutdown can kill its own recovery path.
2. **Detach with `nohup`, a new session, or Phoenix's tmux server** — reduces ordinary shell coupling but remains outside the platform service manager's ownership and does not provide a durable transaction protocol.
3. **Bootstrap a distinct one-shot LaunchAgent with staged immutable inputs** — launchd owns the helper independently, and a manifest can make the disruptive phase deterministic and network-independent.

## Decision

Use a distinct one-shot launchd job for activation. Preparation stages a signed candidate, protected plist, rollback snapshots, hashes, and a redacted immutable manifest before handoff. The helper serializes activation, observes launchd transitions, atomically replaces artifacts, verifies exact embedded identity, and either commits durable source identity or performs verified rollback.

## Consequences

- **Positive:** Phoenix shutdown cannot kill the component responsible for restoring Phoenix; post-handoff correctness does not depend on a terminal, worktree, or network.
- **Positive:** activation and rollback share one explicit state machine with durable outcomes.
- **Negative:** deployment maintains staged transaction artifacts, helper jobs, lock/recovery state, and macOS-specific tests.
- **Negative:** a replacement is successful only after exact runtime verification, so transient startup failures become deployment failures rather than warnings.
- **Neutral:** socket activation still preserves the listener only while the target job is loaded; it does not preserve in-process connections.

## References

- `specs/launchd-deployment/requirements.md`
- `specs/launchd-deployment/launchd-deployment.allium`
- `launchd_prod_deploy`
- `scripts/launchd_deploy_helper.py`
