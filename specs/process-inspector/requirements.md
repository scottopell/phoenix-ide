# Process Inspector

## User Story

As a Phoenix user watching a conversation accumulate background work, I open the
work-scope panel (`specs/work-scope-ui/`) and see a list of bash handles — but a
single row, with its glyph and elapsed time, only tells me *that* a process is
running, not *what it is doing* or *whether it is healthy*. I need to click one
row and get a detailed, live view of that single handle: its identity and exit
state, its output as it scrolls, and a compact resource readout (CPU, memory,
process count) that updates on its own so I can spot a hung build, a runaway
memory climb, or a fork-bomb without dropping to a terminal.

This is the per-handle drill-down that complements the work-scope panel's
roll-up: the panel answers "what is running?"; the inspector answers "what is
*this one* doing, and is it healthy?"

## Domain

A bash handle is the durable record of one backgrounded command, owned by a
`WorkScope` and tracked in the `WorkScope`-keyed bash handle registry
(`specs/bash/` REQ-BASH-WS-001). While live it carries a process group (a `pgid`
and a native `pid`) and an output ring buffer with per-line offsets
(`specs/bash/` REQ-BASH-004); when its process exits it demotes to an in-memory
tombstone holding the exit cause and a final output tail (`specs/bash/`
REQ-BASH-006). The work-scope panel already projects a point-in-time summary of
every handle in a scope (`specs/work-scope-ui/` REQ-WSUI-002).

The inspector is a single-handle, live view rendered in the meta-viewer slot
(`specs/viewer_slot/`) — the right-side surface that hosts exactly one of the
prose reader, the diff viewer, or the live browser view at a time. The inspector
is a fourth viewer kind in that slot. It is launched from a work-scope panel
bash row and addresses one handle by its `(scope_key, handle_id)` pair.

**Transport boundary:** the inspector is a *polling* read view. While open it
fetches a combined snapshot from a pull endpoint about once per second,
incrementally (passing the prior output offset). There is no push transport for
the inspector: the output side is offset-shaped, so a poll-and-tail fits the
existing ring read (`specs/bash/` REQ-BASH-004) without a server-side cursor, and
the resource side requires active sampling that only makes sense while a viewer
is open. Polling stops when the viewer closes or the handle is terminal.

**Persistence boundary:** the inspector introduces no new persistence. Identity,
output, and exit state come from the same registry the work-scope panel reads;
the resource sample is computed at request time and never stored. Everything is
lost on Phoenix restart exactly as the bash handle registry is.

**Resource scope:** the inspector is bash-handle-scoped. It does not inspect the
tmux server or the browser session, which the work-scope panel surfaces at the
roll-up level (`specs/work-scope-ui/` REQ-WSUI-003, REQ-WSUI-004) but which have
no per-process drill-down here (see `design.md`, Non-Goals / Future Work).

---

## Requirements

### REQ-PINSP-001: Combined Inspection Snapshot

WHEN the system assembles a `BashHandleInspection` for a `(scope_key,
handle_id)` pair
THE SYSTEM SHALL produce a single snapshot carrying the handle's identity and
state, an output delta, and a resource sample, drawn only from the in-memory
bash handle registry and a request-time process-group sample
AND SHALL NOT read from or write to any persistent store to do so.

WHEN no handle with `handle_id` exists in the scope keyed by `scope_key`
THE SYSTEM SHALL report this as a not-found condition rather than an empty or
fabricated snapshot.

**Rationale:** One typed snapshot, sourced from the authoritative registry plus
a live sample, keeps the inspector correct-by-construction: there is no second
copy of handle state to diverge from. The identity and state fields are the same
values the work-scope inventory projects (`specs/work-scope-ui/` REQ-WSUI-002),
read from the same registry, so the two surfaces cannot disagree about a handle.
A combined payload lets one poll refresh all three sections — identity, output,
and resources — without three round-trips.

---

### REQ-PINSP-002: Handle Identity and State

WHEN the inspection snapshot is assembled
THE SYSTEM SHALL report the handle's `handle_id`, `label`, display-simplified
`cmd`, `state` (one of `running`, `kill_pending_kernel`, `tombstoned`), and
`started_at`.

WHILE the handle is live (state `running` or `kill_pending_kernel`)
THE SYSTEM SHALL report `pid` and `pgid` sourced from the handle's live data.

WHEN the handle is terminal (state `tombstoned`)
THE SYSTEM SHALL report `exit_code` (when the kernel returned a status code),
`signal_number` (when a signal terminated the process), and `duration_ms` from
the tombstone, and SHALL NOT report a `pid` or `pgid`.

**Rationale:** Identity and state are exactly what the bash tool already tracks
on the handle and tombstone (`specs/bash/` REQ-BASH-006); the inspector reports
the same discrimination — `Live` split into `running` vs `kill_pending_kernel`
by the presence of a recorded kill attempt — that the work-scope inventory uses
(`specs/work-scope-ui/` REQ-WSUI-002). Surfacing `pid`/`pgid` makes a runaway
process actionable; the terminal fields (`exit_code`, `signal_number`,
`duration_ms`) let the inspector show *how* the process ended without the user
re-reading the transcript.

---

### REQ-PINSP-003: Output Delta

WHEN a client requests the inspection snapshot with `since=K`
THE SYSTEM SHALL report the output as `lines` (complete lines whose offset is
in `[max(K, start_offset), end_offset)`), `end_offset`, `truncated_before`, and
the live trailing `partial` (the un-newlined bytes written since the last
newline, or `None` when there is none / for a terminal handle), delegating to
the existing ring read semantics (`specs/bash/` REQ-BASH-004).

WHEN a client requests the inspection snapshot with no `since`
THE SYSTEM SHALL return a recent tail of the output bounded by the default peek
window (`specs/bash/` REQ-BASH-004).

WHEN the handle is terminal
THE SYSTEM SHALL serve the output delta from the tombstone's final tail under
the same offset semantics (`specs/bash/` REQ-BASH-006).

THE output window SHALL be bounded by the handle's output ring (the 4 MB ring
cap while live, the final-tail cap once terminal); the inspector SHALL NOT
retain or assemble scrollback beyond what the ring holds.

**Rationale:** The output side is a thin wrapper over the ring read the bash
tool already exposes (`specs/bash/` REQ-BASH-004). Caller-supplied `since`
offsets keep the server stateless on the read cursor: the client passes `since =
last end_offset` to tail incrementally, and an opening inspector with no `since`
gets a recent tail. `truncated_before` makes ring eviction explicit rather than
silent, exactly as the bash peek does — the inspector does not invent a larger
buffer than the ring provides.

---

### REQ-PINSP-004: Resource Sample — The Core Trio

WHILE the handle is live
THE SYSTEM SHALL report a resource sample over the handle's process *group*
(the `pgid`) comprising:
- `cpu_pct`: the summed CPU percentage over the process group;
- `memory_bytes`: the proportional, shared-aware memory of the group; and
- `process_count`: the number of live processes in the group.

THE `memory_bytes` field SHALL be a proportional, shared-aware measure, defined
per platform as: on Linux, the sum of each group member's
`/proc/<pid>/smaps_rollup` `Pss`; on macOS, the sum of each group member's
`proc_pid_rusage` `RUSAGE_INFO_V2` `ri_phys_footprint`. It SHALL NOT be RSS,
which double-counts shared pages across the group.

WHEN a metric in the trio cannot be sampled on the host platform or kernel
THE SYSTEM SHALL report that field as null
AND SHALL log the capability gap at `debug` level or above.

WHEN the handle is terminal
THE SYSTEM SHALL report the resource sample as absent (there is no process group
to sample).

THE resource sample SHALL be taken only at request time, while a client is
polling an open inspector; the system SHALL NOT sample a handle's process group
in the background.

**Rationale:** The trio answers "is this process healthy?" — CPU for a hung or
spinning process, proportional memory for a leak, process count for a fork
spree. Memory is proportional (PSS / `phys_footprint`) rather than RSS because a
process group sharing libraries or copy-on-write pages would have its shared
pages counted once per member under RSS, inflating the figure; PSS and
`phys_footprint` each attribute shared pages fairly. A null-on-unavailable field
(rather than a misleading zero) with a debug log makes a platform capability gap
visible rather than silent — the same convention the deployment resource sampler
uses (`api/deployment.rs`, `sample_resources`). Sampling only while an inspector
is open bounds the cost to the rare moments an operator is actively watching.

---

### REQ-PINSP-005: Inspection Endpoint

WHEN a client issues `GET /api/work-scope/:scope_key/bash/:handle_id/inspect`
with an optional `since=K` query parameter
THE SYSTEM SHALL return `Json<BashHandleInspection>` for the handle identified
by `handle_id` within the `WorkScope` whose `stable_key()` equals `:scope_key`.

THE endpoint SHALL follow the existing work-scope handler shape (path
parameters, `State(AppState)`, `Json<…>` response), the same shape as
`GET /api/work-scope/:scope_key/inventory` (`specs/work-scope-ui/` REQ-WSUI-006).

**Rationale:** The inspector needs the snapshot at first paint and on every
poll; a plain JSON GET keyed by `stable_key()` plus `handle_id` serves both. It
nests under the existing work-scope route family, reusing the `scope_key`
resolution and disjoint-namespace guarantee that endpoint already relies on. The
`since` query parameter mirrors the ring read's incremental mode (`specs/bash/`
REQ-BASH-004) so the same offset cursor the bash peek uses drives the
inspector's tail.

---

### REQ-PINSP-006: Polling Cadence and Termination

WHILE an inspector is open on a live handle
THE SYSTEM SHALL poll `GET /api/work-scope/:scope_key/bash/:handle_id/inspect`
about once per second, passing `since` equal to the prior response's
`end_offset` for an incremental output tail.

WHEN the inspector's viewer is closed
THE SYSTEM SHALL stop polling.

WHEN the polled snapshot reports the handle as terminal (state `tombstoned`)
THE SYSTEM SHALL render the final snapshot and SHALL stop polling, because a
terminal handle's identity, output tail, and exit state no longer change.

**Rationale:** Roughly once-per-second polling keeps the resource readout and
output tail live without a push transport, which the offset-shaped output and
on-open-only resource sampling do not require for this view. Passing `since =
end_offset` makes each poll an incremental tail rather than a full re-fetch.
Polling stops the moment it can learn nothing more — the viewer is closed or the
handle is terminal — so a closed or finished inspector imposes no recurring
cost.

---

### REQ-PINSP-007: Inspector Viewer Kind

WHEN the user activates the inspect affordance on a work-scope panel bash row
THE SYSTEM SHALL open the process inspector for that handle in the meta-viewer
slot (`specs/viewer_slot/`), closing any other viewer per the slot's
one-at-a-time contract (`specs/viewer_slot/` REQ-VS-007).

THE inspector SHALL be a viewer kind addressed by the handle's `(scope_key,
handle_id)` in the slot's URL contract, so the slot's URL-as-source-of-truth
behaviour (`specs/viewer_slot/` REQ-VS-005, REQ-VS-006) restores the same
inspector on cold reload.

WHEN the user closes the inspector
THE SYSTEM SHALL return to the chat per the slot's close contract
(`specs/viewer_slot/` REQ-VS-004).

WHEN the user switches to a different conversation
THE SYSTEM SHALL close the inspector per the slot's conversation-change reset
(`specs/viewer_slot/` REQ-VS-010).

**Rationale:** The inspector is a per-handle detail surface, which is exactly
what the right-side meta-viewer slot is for. Making it a viewer kind — rather
than an ad-hoc overlay — inherits the slot's mutex, close, conversation-reset,
and URL-restoration behaviour for free, with no parallel view-state machine
(`specs/viewer_slot/`). The `(scope_key, handle_id)` pair is the inspector's
identity, the analogue of prose's file path: encoding it in the URL is what lets
a cold reload restore the same handle's inspector.

---

### REQ-PINSP-008: Inspector Layout and Live Behaviour

WHILE the inspector is open
THE SYSTEM SHALL present three sections: an identity/state header (label, cmd,
state glyph, pid/pgid or exit cause, elapsed/duration), a live-tailing output
pane, and a compact resource readout (`cpu_pct`, `memory_bytes`,
`process_count`).

THE output pane SHALL render in a monospace font, append new lines as polls
arrive, and autoscroll to the newest line WHILE the user has not scrolled up;
WHEN the user scrolls up the pane SHALL pause autoscroll until the user returns
to the bottom.

WHEN the snapshot carries a live `partial`
THE output pane SHALL render it as a trailing in-progress line, visually
distinct from the completed lines and replaced (not appended) on each poll, so
un-newlined output is visible without waiting for a newline or a flush.

THE resource readout SHALL update on each poll (about once per second), and
WHERE a trio metric is null the readout SHALL render it as unavailable rather
than as zero.

THE resource readout SHALL use the shared demand-driven observation generation defined by `specs/deployment-info/` REQ-DEPLOY-007a, while the output pane SHALL retain its independent bash ring-buffer cursor. Opening the inspector SHALL NOT create a second native process sample when a fresh shared generation already contains the handle.

WHEN the snapshot reports `truncated_before`
THE SYSTEM SHALL indicate inline that earlier output fell out of the ring window.

**Rationale:** Per the UI Design Philosophy (information density, inline status,
progressive disclosure): the header answers "what is this and how did/does it
run?"; the output pane answers "what is it doing?"; the resource readout answers
"is it healthy?" — all in one glance, updating live. Autoscroll-with-pause is
the standard log-tailing affordance: follow by default, but never yank a user
who scrolled up to read. Rendering a null metric as unavailable (not zero), and
surfacing `truncated_before`, keeps the absence of data honest rather than
silently misleading — the same correct-by-construction stance the wire's
null-on-unavailable trio takes (REQ-PINSP-004).
