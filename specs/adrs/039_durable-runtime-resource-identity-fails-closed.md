# ADR-039: Durable runtime resource identity fails closed outside proven containment

- **Status:** Accepted
- **Date:** 2026-08-26
- **Affects:** REQ-WL-002b, REQ-WL-002d, REQ-COMP-001

## Context

Close retirement must resume after a Phoenix restart without allowing a stale process-local registry entry, a reused PID, PGID, socket, or profile path to destroy a replacement resource. Phoenix supports macOS and Linux. Both hosts provide per-process birth observations, and tmux provides a Phoenix-controlled durable server token, but neither provides a durable birth identity for a POSIX process group. A bash leader can exit while descendants survive or escape its group, so a leader PID/birth match cannot prove containment of every later group member.

## Options considered

1. **Signal retained PID, PGID, socket, or profile paths after restart** — preserves automatic cleanup in common cases but can target a reused locator or a replacement resource.
2. **Require OS-specific containment for every spawned resource before supporting restart recovery** — could make escaped descendants automatically recoverable on selected hosts, but needs distinct cgroup and macOS containment implementations and expands the product contract beyond Close retirement.
3. **Persist a Phoenix launch marker plus exact leader/server/profile birth identity and fail closed outside proven containment** — supports restart retirement of the proven original instance while leaving ambiguous descendants and replacements untouched in typed repair.

## Decision

Choose option 3.

Phoenix allocates a durable random launch identity before admission and persists normalized resource-instance facts. The record binds the owning WorkScope and resource kind to a Phoenix-controlled marker or stable server/profile authority and exact leader or server process-birth evidence. After restart Phoenix signals, removes, or receipts a resource only when those facts prove the original instance. A replacement at a reused locator is never targeted. If the leader is absent while descendants survive, if a process group may have escaped, or if any marker, containment, or birth observation is incomplete, Phoenix preserves the resource and records `NeedsRepair`.

This decision does not promise automatic cleanup of escaped descendants. Such a guarantee requires a later platform-containment decision and requirement.

## Consequences

- **Positive:** Restart recovery can safely retire a still-proven Phoenix-owned leader, tmux server, or browser/profile instance without PID or path reuse hazards.
- **Positive:** Resource-instance rows preserve explicit evidence and make repair state inspectable rather than relying on volatile registries.
- **Negative:** Some crash-recovery cases remain fenced in `NeedsRepair` and require repair instead of automatic cleanup.
- **Negative:** New resource admissions must create and maintain durable identity rows, with migration and test obligations.
- **Neutral:** In-process generation permits remain useful as a fast exact-instance fence but are not restart authority.

## References

- ADR-003: bash process cleanup uses subreaper plus shutdown kill-tree
- ADR-019: runtime ownership requires positive evidence
- ADR-034: compatibility guarantees are explicit and data-aware
- `specs/work-lifecycle/requirements.md`
- `RuntimeManager::retire_close_runtime_resources`
- `BashHandleRegistry::begin_retirement`
- `TmuxRegistry::inspect_existing_window`
- `BrowserSessionManager::complete_retirement`
