# ADR-118: Bound owned-child settlement after Bash launch

- **Status:** Accepted
- **Date:** 2026-10-17
- **Affects:** `specs/bash/requirements.md` REQ-BASH-017

## Context

The OS can successfully launch Bash before a birth-identity probe returns no result. On macOS, `proc_pidinfo` can return zero or a short result; a short-lived child can also exit before the probe. Calling this `spawn_failed` loses output and invites duplicate execution. A later numeric-PID observation does not prove the original launch identity.

A launch gate could hold user execution until identity was captured, but would add shell protocol, handshake descriptors and sandbox integration to every command. The existing Tokio Child already owns direct-child wait status, including a cached terminal status. Registry admission must not stay pending forever because an unregistered child or its descendants retain a pipe.

## Options considered

1. **Gate execution on birth-identity capture.** Requires a launch protocol, handshake descriptors and sandbox integration on every command.
2. **Retain the owned Child for bounded settlement.** Preserves direct-child output and wait status without inventing a process identity or keeping registry admission pending indefinitely.

## Decision

Use owned-child settlement, not a launch gate. Before the first await after spawn, transfer the Child and admission reservation into a caller-independent task. Bound registration to one second. On post-launch failure, retain bounded ring readers while allowing 250ms for natural completion. This deliberately short failure-path grace bounds unregistered execution; it is distinct from the one-second registration and reap deadlines. Read cached status via `try_wait`; if still live, request termination with the held Child's `start_kill`, then bound the reap wait to one second. Drain descendant-held pipes for 200ms only after direct-child terminal status; if settlement expires first, close capture as explicitly incomplete.

Tokio 1.52.3 routes `Child::start_kill` through `FusedChild`: completed children are a no-op, and live children retain their unreaped process object. Unix `Reaper` delegates kill to that held std Child; it does not hand a live owned child to the orphan queue. Successful kill disables kill-on-drop. Dropping an unreaped Child delegates reaping to Tokio's orphan queue. This permits direct-child termination without fabricating an identity or signaling a numeric PID/process group through Phoenix. A wait error does not authorize a new explicit signal attempt.

Never re-probe identity for adoption, signal an unknown group, or replay. Return handle-free typed terminal results when owned wait proves terminal status, otherwise an explicit side-effect-unknown error prohibiting automatic retry. Descendant shutdown remains unverified in both cases.

Release pending admission after bounded settlement. Retain one process-local uncertainty bit per owner, fencing subsequent spawns and entering the existing retirement failure report. The report carries typed scope uncertainty separately from process-group failures, without inventing a group ID or signal authority. Its `has_failures` check causes Close's `require_absent` check to reject successful retirement, including when no exact process-group target exists. Preserve sandbox scratch that unverified descendants might still use. No persistence, restart recovery, historical PID authority or auto-cleanup is added.

## Consequences

A live identity-unavailable command may be terminated rather than run indefinitely without a control handle. This is an explicit launch-failure policy, not a `wait_seconds` timeout. A verified direct-child exit does not imply safe retry or descendant absence. An unregistered launch conservatively fences its scope until process restart; pending admission and teardown nevertheless settle finitely with an actionable failure.

Caller cancellation during registration no longer drops the temporary process-group guard: ownership is transferred synchronously before that await. This does not claim to repair all pre-existing process-group Drop, normal registered-handle signaling, runtime-shutdown or escaped-descendant behavior.

Regression coverage lives in `crates/phoenix-tools/src/bash/operations.rs::tests` and `crates/phoenix-core/src/process_identity.rs::tests`. `.github/workflows/bash-launch.yml` runs the focused faults on hosted Linux/macOS and publishes generated wire contracts; the existing full CI covers UI/schema integration. Qualification status belongs in `specs/bash/executive.md`. No production action is part of this decision.
