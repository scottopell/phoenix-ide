# ADR-228: Coordinator inline children retire their Bash controllers before parent acceptance

- **Status:** Accepted
- **Date:** 2026-11-04
- **Affects:** REQ-GR-007, REQ-BASH-017, REQ-BASH-018, REQ-SA-003, REQ-SA-004, REQ-SA-005

## Context

ADR-227 gives trusted filesystem-free Coordinator children the existing
unsandboxed, explicitly WorkScope-targeted Bash tool. Each command belongs to
its selected WorkScope, while the child's own Unattached resource identity
controls its handle. Ordinary child-resource cleanup follows the child's
resource scope, so it misses background Bash stored in those selected owner
scopes. A child can terminate while its commands continue without a remaining
authorized agent to manage them.

Launch gating also separates handle registration from command-payload release.
A controller cleanup that snapshots registered handles before admitted launch
custody settles can miss a later registration or race a pending payload release.
Persisted child terminal evidence can reach parent reconciliation independently
of live terminal-result transport.

## Options considered

1. **Tear down selected WorkScopes.** Reuses owner cleanup but kills other
   controllers' commands and can destroy a live conversation's environment.
2. **Transfer child handles to the Coordinator.** Leaves background commands
   manageable but expands parent control and makes child termination depend on
   subsequent manual intervention.
3. **Retire the child's controller independently of its owner scopes.** Adds a
   narrow process-local admission and cleanup boundary while preserving the
   capability and ownership decision in ADR-227.

## Decision

Choose option 3. This completes ADR-227's lifecycle without changing its trusted
write-capable Bash policy or giving children an ambient cwd or WorkScope
attachment. Apply it only to filesystem-free inline Coordinator children;
ordinary Work children retain their existing lifecycle.

Close controller admission atomically, retain the closed gate for the process
lifetime, and wait for admitted launches through registration and gated payload
release or bounded unregistered settlement. Then snapshot only that controller's
registered live handles across their owning WorkScopes and perform bounded
incarnation-bound SIGKILL attempts and available exit observation.

The exact child identity owns one cancellation-resistant operation. Terminal
paths and the parent-cancellation backstop start or join it; cancellation of a
requester cannot abandon custody or reopen admission. Reuse existing Bash launch
settlement bounds and one shared registered-handle shutdown budget. Both live
delivery and durable-result parent acceptance join its bounded observation.
Incomplete observation permits fan-in with logged uncertainty and retained
handle/waiter evidence; it cannot certify successful cleanup.

Controller retirement leaves handles in their selected WorkScope tables through
normal waiter-driven terminalization and tombstone retention, including on
signaling failure. It neither tears down those owners nor affects another
controller's handles, owner inventory/caps, or existing owner-admission rules.

## Consequences

- **Positive:** A child's terminal boundary fences late launches and owns cleanup
  independently of target selection, caller cancellation, and result transport.
- **Positive:** WorkScope inventory and inspection retain evidence of commands
  whose signaling or exit observation is incomplete.
- **Negative:** Bounded, best-effort cleanup can leave live resources visible
  after parent fan-in. Successful signaling or leader exit cannot establish
  complete descendant absence or process-group reaping.
- **Neutral:** Closed controller gates remain process-local. This adds no durable
  launch identity, restart cleanup authority, compatibility promise, raw PID/PGID
  signaling fallback, or general process-tree reaper.

## References

- [ADR-227: trusted explicitly targeted Bash](227_coordinator-inline-agents-reuse-targeted-bash.md)
- [Coordinator capabilities](../global-recall/requirements.md)
- [Bash lifecycle requirements](../bash/requirements.md)
- [Bash lifecycle contract](../bash/bash.allium)
- [Sub-agent terminal delivery and acceptance](../subagents/subagents.allium)
- `WorkScopeCoordinatorBash`, `SpawnReservation`, `BashHandleRegistry`
