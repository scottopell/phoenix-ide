# Process Inspector — Executive Summary

## Requirements Summary

A detailed, live view of a single backgrounded bash handle, opened from a
work-scope panel bash row (`specs/work-scope-ui/`) and rendered in the right-side
meta-viewer slot (`specs/viewer_slot/`) — the same slot as the prose reader, diff
viewer, and browser view, only one active at a time. It answers "what is this
process doing, and is it healthy?" at a glance: an identity/state header, a
live-tailing output pane, and a compact resource readout (CPU, memory, process
count) that refreshes on its own.

The inspector is a polling read view. While open it fetches a combined snapshot
from `GET /api/work-scope/:scope_key/bash/:handle_id/inspect?since=K` about once
per second, passing the prior output offset for an incremental tail. Polling
stops when the viewer closes or the handle is terminal (then it shows the final
snapshot — there is nothing more to learn). There is no push transport: the
output is offset-shaped (a thin wrapper over the existing bash ring read,
`specs/bash/` REQ-BASH-004), and the resource sample is taken only while a viewer
is open, so neither side benefits from server push for this view.

## Technical Summary

`BashHandleInspection` (defined in `phoenix-core::domain`, ts_rs-exported like
the work-scope inventory types) is the combined wire projection: identity/state
(`handle_id`, `label`, `cmd`, `state`, `pid`, `pgid`, `started_at`, and
`exit_code` / `signal_number` / `duration_ms` when terminal), an output delta
(the reused `tool_wire::BashRingWindow` — `lines`, `end_offset`,
`truncated_before`), and an `Option<ResourceSample>` (`cpu_pct`, `memory_bytes`,
`process_count`, each independently nullable). `state` reuses the inventory's
`BashHandleState` enum so the two surfaces share one state vocabulary.

The snapshot is assembled in the `tools` layer (parallel to
`work_scope_inventory::assemble_inventory`), reading the `WorkScope`-keyed bash
handle registry through its non-creating accessors. Output reuses the bash ring
read (`RingBuffer::since` / `tail`, the tombstone final-tail read). The resource
trio is sampled over the handle's process *group*: `sysinfo` (already a
dependency) covers CPU% and process enumeration, while proportional memory and
group membership are platform-specific custom reads — Linux `/proc`
(`smaps_rollup` `Pss`, scan-by-pgrp for membership), macOS `proc_listpgrppids` +
`proc_pid_rusage` `ri_phys_footprint`. Memory is proportional (PSS /
`phys_footprint`), not RSS, which double-counts shared pages. Sampling happens
only at request time while an inspector polls; an unavailable metric is null
(not zero) and logged at `debug`, per the codebase's capability-gap convention.

The handler nests under the existing `/api/work-scope/:scope_key/…` route family
established by `GET /api/work-scope/:scope_key/inventory`. No SSE wire variant is
added, so the SSE cross-spec whitelist (`specs/sse_wire/`) is untouched.

On the client the inspector is a new `inspect` `ViewerKind` in the meta-viewer
slot (`ui/src/contexts/ViewerSlotContext.tsx`), addressed by `(scope_key,
handle_id)` in the slot's URL contract so a cold reload restores the same
handle's inspector. It inherits the slot's one-at-a-time mutex, close,
conversation-change reset, and URL-restoration behaviour. A "view" affordance on
the work-scope panel's bash row opens it; a `ProcessInspectorPanel` owns the poll
loop and renders the header, the autoscroll-with-pause output pane, and the
resource readout.

No Allium spec accompanies this spec: the inspector is a read-projection plus a
client-side poll over state whose lifecycles are already modeled by `specs/bash/`
(`bash.allium`, the handle / ring / tombstone) and `specs/viewer_slot/`
(`viewer_slot.allium`, the slot mutex and URL contract). It adds no new state
machine, precondition, or ordering obligation, so spEARS alone is the correct
weight (see `design.md`, "Why No Allium Spec"), mirroring `specs/work-scope-ui/`.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-PINSP-001:** Combined Inspection Snapshot | Proposed | One `BashHandleInspection` from the registry + a request-time sample; not-found when the handle is absent |
| **REQ-PINSP-002:** Handle Identity and State | Proposed | Reuses the inventory's `BashHandleState`; adds tombstone `exit_code` / `signal_number` (`specs/bash/` REQ-BASH-006) |
| **REQ-PINSP-003:** Output Delta | Proposed | Thin wrapper over the ring read (`specs/bash/` REQ-BASH-004); reuses `tool_wire::BashRingWindow` |
| **REQ-PINSP-004:** Resource Sample — The Core Trio | Proposed | `cpu_pct` / `memory_bytes` (PSS/phys_footprint, not RSS) / `process_count` over the `pgid`; null-on-gap, logged at debug; sampled only while open |
| **REQ-PINSP-005:** Inspection Endpoint | Proposed | `GET /api/work-scope/:scope_key/bash/:handle_id/inspect?since=K`; mirrors the inventory handler shape |
| **REQ-PINSP-006:** Polling Cadence and Termination | Proposed | ~1s poll with `since = end_offset`; stops on close or terminal handle |
| **REQ-PINSP-007:** Inspector Viewer Kind | Proposed | New `inspect` `ViewerKind` keyed by `(scope_key, handle_id)`; inherits the slot's mutex / close / reset / URL restore (`specs/viewer_slot/`) |
| **REQ-PINSP-008:** Inspector Layout and Live Behaviour | Proposed | Header + autoscroll-with-pause output pane + resource readout; null metric renders unavailable, `truncated_before` shown inline |

**Progress:** 0 of 8 implemented.
