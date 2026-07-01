# ADR-002: Bash handle state uses watch-backed exit notifications and snapshot shaping

- **Status:** Accepted
- **Date:** 2026-06-29
- **Affects:** REQ-BASH-001, REQ-BASH-003, REQ-BASH-004, REQ-BASH-005, REQ-BASH-006, REQ-BASH-014, REQ-BASH-WS-002

## Context

A bash handle has two different jobs at once: it has to observe process exit
reliably, and it has to shape responses from whichever state is current when the
agent asks. The design material describes a single, in-memory handle state that
is shared by `peek`, `wait`, `run`, and `kill`, with a terminal demotion path on
exit.

The same material also calls out two specific implementation pressures:

- the exit notification must be observable by late waiters, not just tasks that
  were already parked when the exit happened
- response shaping needs to read the current state without tying the whole tool
  surface to a live write lock or a persisted store

## Options considered

1. **`Notify` for exit notification, plus `ArcSwap` for response snapshots** —
   wake waiters with `Notify` and publish state through `ArcSwap`. This would fit
   an event style, but `Notify` only wakes tasks already waiting, so a late
   waiter can miss the transition. The design material explicitly rejects that
   footgun.
2. **`watch` for exit notification, plus `RwLock<Arc<HandleState>>` snapshots**
   — use a `tokio::sync::watch::channel` so state transitions remain observable
   to late subscribers, and keep the response-shaping state in an
   `RwLock<Arc<HandleState>>` so callers can take a stable snapshot.
3. **SQLite-persisted handle state** — store handle state durably in the
   database. This would shift the handle model away from the in-memory tool
   lifecycle the design material describes and would add persistence work that
   bash itself does not need.

## Decision

Use watch-backed exit notifications and `RwLock<Arc<HandleState>>` snapshots for
response shaping. The watcher makes exit transitions observable to late
subscribers, while the `RwLock` around an `Arc` lets callers shape a response
from a consistent snapshot without making the state persist beyond the Phoenix
process.

This keeps process exit observation reliable, keeps the response path local to
memory, and avoids turning bash into a persistence system.

## Consequences

- **Positive:** late `wait` callers can still observe the terminal transition.
- **Positive:** response shaping can read a stable `Arc<HandleState>` snapshot
  without requiring a persisted record.
- **Positive:** the handle state stays in-memory and aligns with the rest of the
  bash registry.
- **Negative:** the design depends on Tokio watch semantics, so the exit path
  must publish the transition exactly once.
- **Negative:** `ArcSwap` and SQLite persistence stay out of the bash state
  path, so any future need for durable cross-restart state has to be handled by
  a different subsystem.
- **Neutral:** the watch receiver is cloned per call, which matches the existing
  wait/run call shape.

## References

- Related ADRs: ADR-001
- Feature spec: `specs/bash/requirements.md`
- Behavioural spec: `specs/bash/bash.allium`
- Executive summary: `specs/bash/executive.md`

