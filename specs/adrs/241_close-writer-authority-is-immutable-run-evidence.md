# ADR-241: Close writer authority is immutable run evidence

- **Status:** Accepted
- **Date:** 2026-11-06
- **Affects:** REQ-WL-002b, REQ-WL-002c, REQ-WL-002e

## Context

Close must preserve worktrees when an external process retains write authority or attributable process inspection is incomplete. A boolean or transient error loses which process incarnation, capability, detector and exact retirement authority produced the blocker. PID reuse and fresh safe-retry runs make an unbound process record misleading.

## Options considered

1. Keep only a transient blocker string. Small, but not durably auditable.
2. Embed observations in an inspection JSON collection or mirror them into a UI payload. This duplicates authority and prevents relational enforcement of child bindings and immutability.
3. Persist normalized immutable observations under the admitted Close run, with exact inspection and resource binding. This preserves audit evidence without adding process-control or browser surfaces.

## Decision

Use option 3. Migration 153 adds `close_writer_observations` and schema-enforced positive-authority versus inspection-gap shapes. The child key is attempt, admitted run ordinal, scope and detector; it records the first blocking observation for that detector in that exact run. An identical replay is a no-op. A conflicting observation, including changed incarnation behind the same PID, cannot overwrite history. A fresh admitted ordinal can retain its own evidence. Immutability lasts for the owning aggregate's lifetime: only the owning product's actual deletion cascade removes observations. A direct product foreign key and AFTER DELETE guard reject child deletion while that product still exists, including an empty History product. Deferred run/dispatch foreign keys allow the existing teardown ordering but reject committed orphans; no generic bypass flag is introduced.

Positive evidence distinguishes directory-descriptor path-relative authority, writable ordinary file descriptors, cwd authority and writable shared mappings. A read-only ordinary file is not a writer; movement of its read position is not identity change. Unavailable process inventory and incomplete attributable evidence fail closed. Ambient access denial is not itself positive writer evidence. Linux single-task zombies are excluded before executable inspection only after unambiguous State/Threads fields, a matching singleton task inventory and a stable status recheck. Zombie leaders with siblings or uncertain task inventories, malformed state and live-to-zombie changes fail closed.

Persistence occurs before an admitted retirement probe returns a blocker. Failure to persist preserves the target. Pre-admission probes have no admitted run under which to write. These records never authorize terminating an external process. No UI or HTTP API is introduced, and no compatibility or recovery guarantee is expanded.

## Consequences

- **Positive:** immutable SQL child rows retain exact attribution across failed runs, retries and PID reuse; duplicate `INSERT OR REPLACE` cannot bypass immutability when recursive triggers are disabled.
- **Negative:** evidence-write failure blocks cleanup; an unexpected second, different observation at the same detector key fails rather than silently revising evidence.
- **Neutral:** production syscall qualification remains separate from deterministic fake-process tests. `Database::list_close_writer_observations` returns the existing typed request with exact run, scope, snapshot and resource binding; strict column decoding preserves Unix native path bytes and rejects malformed evidence. This decision does not require exposing evidence to a browser.

## References

- [Work lifecycle requirements](../work-lifecycle/requirements.md), REQ-WL-002e.
- [Work lifecycle behavior](../work-lifecycle/work-lifecycle.allium), `AmbiguousPrivateDirectoryIdentityPreservesSafeLeftover`.
- `Database::record_close_writer_observation`, `CloseWriterProbe::retain`, `linux_writer_scan`.
- [ADR-034](034_compatibility-guarantees-are-explicit-and-data-aware.md): no additional compatibility policy.
