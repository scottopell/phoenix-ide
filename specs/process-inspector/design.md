# Process Inspector — Design Document

## Architecture Overview

The inspector is a per-handle live view over a single bash handle, assembled on
demand from the `WorkScope`-keyed bash handle registry plus a request-time
process-group sample, served by a pull endpoint and rendered as a new kind in
the meta-viewer slot.

```
   bash handle registry ─┐
   ring read (REQ-BASH-004)┤
   pgid resource sample ──┼─► assemble BashHandleInspection(scope, handle, since)
                          │                     │
                          │         GET …/bash/:handle_id/inspect?since=K   (pull)
                          │                     │
                          │            ~1s poll while viewer open
                          │                     │
                          └──► ProcessInspectorPanel (meta-viewer slot kind=inspect)
```

There is one carrier of each value: the registry owns identity, state, and
output; the resource sample is computed fresh per request and never stored;
`BashHandleInspection` is the wire projection. The frontend holds only the slot
identity (`scope_key`, `handle_id`) in the URL and the latest polled snapshot in
component-local state. No layer keeps a second, separately-updated copy.

The inspector deliberately reuses, rather than re-derives:

- **Identity and state** come from the same `Handle` / `Tombstone` the
  work-scope inventory reads (`specs/work-scope-ui/` REQ-WSUI-002), via the same
  `Live` → `running`/`kill_pending_kernel` discrimination.
- **Output** is the existing ring read (`specs/bash/` REQ-BASH-004); the
  inspection payload embeds the same `start_offset` / `end_offset` /
  `truncated_before` / `lines` window shape the bash peek returns.
- **The viewer surface** is the existing meta-viewer slot
  (`specs/viewer_slot/`); the inspector is a fourth `ViewerKind`, inheriting the
  slot's mutex, close, reset, and URL-restoration behaviour.

## Why No Allium Spec

Allium is warranted for state machines, lifecycle flows with preconditions, and
multi-step operations where ordering matters (`AGENTS.md`, "When to write an
Allium spec"). The inspector has none of those *of its own*. The lifecycle it
observes — a bash handle moving `running` → `kill_pending_kernel` → `tombstoned`,
its ring buffer filling and evicting — is already modeled by `specs/bash/`
(`bash.allium`), which owns those transitions, preconditions, and invariants. The
viewer-slot mutex, open/close transitions, and URL-restoration contract the
inspector mounts into are already modeled by `specs/viewer_slot/`
(`viewer_slot.allium`), and the inspector adds one more `ViewerKind` value to a
union that spec already governs.

What the inspector itself adds is a *read-projection* (`BashHandleInspection`)
over already-modeled state plus a client-side *poll loop*. It introduces no new
persisted state machine, no new precondition beyond "the handle exists in the
registry," and no new ordering obligation beyond "the snapshot reflects the
registry and a process sample at request time." An Allium spec would restate the
bash-handle and viewer-slot lifecycles it does not own. spEARS alone is the
correct weight here, mirroring the justification in `specs/work-scope-ui/design.md`
("Why No Allium Spec") for the sibling roll-up panel.

## Data Model

### `BashHandleInspection`

The wire projection, defined in `phoenix-core::domain` alongside the other
wire-projection/domain types (the same layer as
`phoenix_core::domain::work_scope_inventory::WorkScopeInventory` and
`phoenix_core::domain::tool_wire`), so the `tools` and `api` layers can depend
*down* onto it. Assembled on demand; never stored.

```text
BashHandleInspection {
    // ── identity / state ──────────────────────────────────────────
    handle_id:       String,              // Handle.handle_id
    label:           Option<String>,      // skipped when absent
    cmd:             String,              // display-simplified
    state:           BashHandleState,     // running | kill_pending_kernel | tombstoned
                                          //   (reused from work_scope_inventory)
    pid:             Option<u32>,         // present while live
    pgid:            Option<i32>,         // present while live
    started_at:      DateTime<Utc>,       // RFC3339 on the wire
    exit_code:       Option<i32>,         // present when terminal & kernel returned a code
    signal_number:   Option<i32>,         // present when terminal & signal-terminated
    duration_ms:     Option<u64>,         // present only when terminal

    // ── output delta (REQ-BASH-004 ring read) ─────────────────────
    output:          BashRingWindow,      // reused from tool_wire:
                                          //   { start_offset, end_offset,
                                          //     truncated_before, lines: [{offset, bytes}] }

    // ── resource sample (live only; null per-field on capability gap) ─
    resources:       Option<ResourceSample>,   // None when terminal
}

ResourceSample {
    cpu_pct:         Option<f32>,         // summed CPU% over the pgid tree; null if unavailable
    memory_bytes:    Option<u64>,         // PSS (Linux) / phys_footprint (macOS); null if unavailable
    process_count:   Option<u32>,         // live processes in the group; null if unavailable
}
```

`state` is the existing
`phoenix_core::domain::work_scope_inventory::BashHandleState`
(`running | kill_pending_kernel | tombstoned`) — reused, not redefined, so the
inspector and the work-scope inventory cannot disagree on a handle's state
vocabulary. `output` is the existing
`phoenix_core::domain::tool_wire::BashRingWindow` (with `BashRingLine`), reused
verbatim from the bash tool's response shape so the inspector's output window is
byte-for-byte the peek window.

`ResourceSample` is the new type the inspector introduces. It is `Option` on the
parent (`None` for a terminal handle, which has no process group) and each of
its three fields is independently `Option` (null when that specific metric is
unavailable on the host) — the two levels of optionality are distinct:
`resources = None` means "no group to sample," `resources.cpu_pct = null` means
"a group exists but this metric could not be read here."

### Why `exit_code` / `signal_number` here but not in the inventory

The work-scope inventory's `BashHandleInventory` carries `duration_ms` for a
terminal handle but not `exit_code` / `signal_number` (`specs/work-scope-ui/`
design): the roll-up panel shows *that* a handle finished, with its glyph and
elapsed time, not *how*. The inspector is the drill-down, so it adds the exit
cause. These come from the same `Tombstone` fields (`exit_code`,
`signal_number`, `duration_ms`; `specs/bash/` REQ-BASH-006) — the inspector
reads two fields the inventory chose not to project, not a parallel
representation.

## Sourcing the Snapshot

The assembly lives in the `tools` layer (next to
`phoenix_tools::work_scope_inventory::assemble_inventory`, which it parallels),
because that layer has access to the registry types `phoenix-core` cannot depend
on. It is a read-only path: the registry is queried through its non-creating
`get_existing(work_scope)` accessor and the table's `get(&HandleId)` lookup, so
inspecting a handle never allocates one.

### Identity / state / output

For the resolved `Arc<Handle>`:

- `handle_id`, `label`, `cmd`, `started_at` are fields on `Handle`.
- `state` discriminates `HandleState::Live` (further split `running` vs
  `kill_pending_kernel` by `Handle::kill_attempt().await.is_some()`, the same
  rule `project_handle` uses in `work_scope_inventory.rs`) from
  `HandleState::Tombstoned`.
- `pid`, `pgid` come from `LiveData` while live (the `Handle::live_pid()` /
  `Handle::live_pgid()` accessors, or directly off the held `LiveData`).
- `exit_code`, `signal_number`, `duration_ms` come from the `Tombstone` when
  terminal.
- `output` comes from the live ring's `since` / `tail` read while live and from
  the tombstone's final-tail read when terminal, both already implemented in
  `phoenix-tools` bash `operations.rs` (`read_window_from_ring` /
  `read_window_from_tombstone`) and `ring.rs` (`RingBuffer::since` /
  `RingBuffer::tail`). The inspector calls the same window read and maps the
  resulting `WindowView` to `BashRingWindow` exactly as the bash tool does
  (`window_to_typed`).

### Resource sample (the core trio)

The sample is taken over the live handle's `pgid` and is platform-specific. Two
distinct concerns are involved at each layer:

1. **Group membership** — which pids belong to the `pgid`:
   - Linux: scan `/proc/<pid>/stat` (or `/proc` entries) for processes whose
     process-group id equals `pgid`.
   - macOS: `proc_listpgrppids(pgid, …)` (via `libc`) returns the group's pids.
2. **Per-pid metrics**:
   - `cpu_pct` and `process_count`: `sysinfo` (already a dependency in
     `phoenix-ide`) enumerates processes and reports per-process CPU%; summing
     the group members' CPU% gives the trio's `cpu_pct`, and counting the live
     members gives `process_count`. CPU% needs two samples separated by
     `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`, the same two-refresh pattern
     `api/deployment.rs::sample_resources` uses.
   - `memory_bytes` (proportional / shared-aware) is **not** `sysinfo`'s
     `Process::memory()`, which is RSS and double-counts shared pages. It is a
     custom per-platform read:
     - Linux: sum `/proc/<pid>/smaps_rollup` `Pss` over the group.
     - macOS: sum `proc_pid_rusage(pid, RUSAGE_INFO_V2, …)`'s `ri_phys_footprint`
       over the group (via `libc`).

`sysinfo` covers CPU% + enumeration; PSS / `phys_footprint` and `pgid`-group
membership are the platform-specific custom reads `sysinfo` does not expose.
`libc` is already a dependency of `phoenix-tools` and `phoenix-core`, and
`sysinfo` of `phoenix-ide`, so no new dependency is required for either side.

The sample is computed **only at request time**, while the inspector is polling
(~1s). There is no background sampler: a handle's process group is read exactly
when an operator is looking at it. This bounds the cost — including the
two-sample CPU interval sleep — to the open-inspector window.

When a metric cannot be read on the host (no `smaps_rollup` on an old kernel,
`proc_pid_rusage` failing, a pid that exited between enumeration and read), that
field is `null` and the gap is logged at `debug` — capability-gap visibility per
the codebase's correct-by-construction conventions (`AGENTS.md`, "Capability
gaps are logged, not silenced"), the same null-not-zero stance
`sample_resources` takes for unavailable host metrics.

## Pull Endpoint (REQ-PINSP-005)

```
GET /api/work-scope/:scope_key/bash/:handle_id/inspect?since=K
    → Json<BashHandleInspection>
```

The handler mirrors `get_work_scope_inventory` in `api/handlers.rs`: it takes
`State(AppState)`, `Path((scope_key, handle_id))`, and `Query` for the optional
`since`. It resolves the `WorkScope` via `WorkScope::from_stable_key(&scope_key)`
(a `400` on a malformed key, exactly as the inventory handler does), looks up the
handle in that scope's table by `handle_id`, assembles the
`BashHandleInspection` (identity + state + output window for `since` + resource
sample when live), and returns it as JSON. A missing scope or handle is a
not-found condition (REQ-PINSP-001).

The route nests under the existing `/api/work-scope/:scope_key/…` family that
`GET /api/work-scope/:scope_key/inventory` (`specs/work-scope-ui/` REQ-WSUI-006)
established, reusing the `stable_key()` resolution and the disjoint-namespace
guarantee that a `worktree:` key cannot resolve a `conversation:`-scoped
resource.

There is **no SSE/push variant** for the inspection payload. The work-scope
roll-up panel uses a push (`WorkScopeUpdate`, `specs/work-scope-ui/`
REQ-WSUI-007) because it is edge-triggered on coarse resource state changes; the
inspector instead polls because its two live signals — incremental output tail
and an active process-group resource sample — are poll-shaped, and a full push
of the resource trio would require a background sampler the inspector
deliberately avoids (REQ-PINSP-004). A push of finalized output is a later
optimization (see Non-Goals / Future Work), not a v1 requirement.

## Frontend

### Viewer kind (REQ-PINSP-007)

The meta-viewer slot (`ui/src/contexts/ViewerSlotContext.tsx`) is a discriminated
union `ViewerSlot` over `kind ∈ {none, prose, diff, browser}`, derived from the
URL `?viewer=` param on every render (`specs/viewer_slot/` REQ-VS-002,
REQ-VS-007). The inspector adds a fifth variant:

```text
| { kind: 'inspect'; scopeKey: string; handleId: string }
```

with the URL shape `?viewer=inspect&scope=<scope_key>&handle=<handle_id>`. The
`(scopeKey, handleId)` pair is the inspector's slot identity, the analogue of
prose's `{ path, rootDir }`: it is what `deriveSlot` reads back from the URL so a
cold reload restores the same handle's inspector (`specs/viewer_slot/`
REQ-VS-005, REQ-VS-006). A `?viewer=inspect` URL missing `scope` or `handle` is
malformed and normalizes to `none`, the same defensive path
`UrlMalformedNormalizesToNone` applies to prose-without-file
(`specs/viewer_slot/`). An `openInspect(scopeKey, handleId)` action on the slot
provider write-throughs to the URL with `{ replace: true }`, like the existing
`openProse` / `openDiff`. The slot's mutex (`specs/viewer_slot/` REQ-VS-007)
means opening the inspector closes any prose/diff/browser viewer; closing it
clears the params and returns to chat (REQ-VS-004); a conversation switch resets
it (REQ-VS-010) — all inherited, no new view-state machine.

Per the slot's per-conversation persistence (`specs/viewer_slot/` REQ-VS-014),
the `inspect` URL snapshot is restored on in-app re-entry like any other viewer
kind, since that storage round-trips the opaque URL params.

### Launch affordance (REQ-PINSP-007)

The work-scope panel's bash row (`BashRow` in
`ui/src/components/WorkScopePanel.tsx`) gains an inspect affordance — a "view"
control distinct from the row's existing inline detail toggle. Activating it
calls `openInspect(scopeKey, handle.handle_id)` from the slot provider
(`useViewerSlot`), passing the panel's `scopeKey` and the row's `handle_id`.
This is the same launch pattern as opening prose from a file row: a panel row's
click resolves to a slot-open call.

### Inspector panel (REQ-PINSP-006, REQ-PINSP-008)

A new `ProcessInspectorPanel` component renders when `slot.kind === 'inspect'`,
mounted in the right-side viewer surface beside the prose/diff/browser viewers.
It owns the poll loop and the three sections:

- **Poll loop.** On mount and while `slot.kind === 'inspect'`, it fetches
  `GET /api/work-scope/:scope_key/bash/:handle_id/inspect?since=<lastEndOffset>`
  about once per second, seeding `since` from the prior response's `end_offset`
  for an incremental tail and appending the returned `lines`. The first fetch
  omits `since` to load a recent tail. It stops on unmount (viewer closed) and
  when a response reports `state === 'tombstoned'`, rendering the final snapshot
  thereafter (REQ-PINSP-006). The generated type
  (`ui/src/generated/BashHandleInspection.ts`) types the response.
- **Identity/state header.** Label, display `cmd`, a state glyph (green running,
  yellow `kill_pending_kernel`, muted tombstoned — the `bashGlyph` vocabulary
  the panel already uses), `pid`/`pgid` while live or `exit_code` /
  `signal_number` when terminal, and elapsed (live) / `duration_ms` (terminal).
- **Output pane.** Monospace, line-buffered, appends `lines` as polls arrive.
  Autoscrolls to the newest line while the user is at the bottom; pauses
  autoscroll when the user scrolls up and resumes when they return to the
  bottom. Surfaces `truncated_before` inline as an "earlier output evicted"
  marker (REQ-PINSP-008).
- **Resource readout.** A compact `cpu_pct` / `memory_bytes` / `process_count`
  line, updated each poll. A null field renders as unavailable (an em-dash or
  "n/a"), never as `0` (REQ-PINSP-004, REQ-PINSP-008). When `resources` is
  `None` (terminal handle), the readout is omitted entirely.

The slot's URL-driven render isolation means the inspector mounts and unmounts
on slot-kind transitions without churning the chat column, the same way the
prose and diff viewers do (`specs/viewer_slot/`).

## Wire-Shape Notes

- **`started_at`** is `Handle.started_at`, a `SystemTime` rendered as
  `DateTime<Utc>` → an RFC3339 string on the wire (not `i64`), matching the
  `BashHandleInventory.started_at` convention (`specs/work-scope-ui/` design).
- **`pid` / `pgid` / `resources`** are `Option`, present only while the handle
  is live; skipped via `skip_serializing_if = "Option::is_none"` so they render
  as `undefined` on the TS side (the schema treats them as optional), the same
  optional-while-live pattern `BashHandleInventory` uses for `pid`/`pgid`.
- **`exit_code` / `signal_number` / `duration_ms`** are `Option`, present only
  when terminal; `exit_code` is null when a signal terminated with no code and
  `signal_number` is null when the kernel returned a code — the exact tombstone
  shape from `specs/bash/` REQ-BASH-006.
- **`output`** is the existing `BashRingWindow` (`tool_wire.rs`): `start_offset`,
  `end_offset` (`u64`), `truncated_before` (`bool`), `lines: [{ offset: u64,
  bytes: String }]`. `bytes` is lossy-UTF-8 line contents, as the bash peek
  emits.
- **`ResourceSample` fields** are each `Option`: `cpu_pct: Option<f32>`,
  `memory_bytes: Option<u64>`, `process_count: Option<u32>`. A null is a real
  capability gap, distinct from a `0` sample, and skipped on the wire so the TS
  side sees `undefined`.

## TypeScript Codegen (per `AGENTS.md` "TypeScript codegen for SSE types")

`BashHandleInspection` and the new `ResourceSample` carry
`#[derive(ts_rs::TS)]` + `#[ts(export, export_to = "../../../ui/src/generated/")]`,
the same annotation `WorkScopeInventory` and `BashRingWindow` already carry, so
`cargo test` emits `ui/src/generated/BashHandleInspection.ts` and
`ui/src/generated/ResourceSample.ts`. The reused types
(`BashHandleState`, `BashRingWindow`, `BashRingLine`) are already exported there;
the inspection type references them. After adding the types, `./dev.py codegen`
regenerates the files and `./dev.py check` fails on a stale
`git diff -- ui/src/generated/`. The generated files are never hand-edited.

Because the inspection payload travels over a plain JSON GET (not the SSE wire),
it needs no `SseWireEvent` variant and no valibot schema in `ui/src/sseSchemas.ts`
— the SSE-variant cross-spec whitelist in `specs/sse_wire/` (`AUTHORING.md` §7)
is therefore untouched by this spec.

## Files to Create / Modify

### New

- `phoenix-core::domain`: `BashHandleInspection` + `ResourceSample`, with
  `ts_rs` derives, referencing the existing `BashHandleState`
  (`work_scope_inventory`) and `BashRingWindow` (`tool_wire`).
- `phoenix-tools`: an `assemble_inspection` function (parallel to
  `work_scope_inventory::assemble_inventory`) plus the platform-specific
  process-group sampler (Linux `/proc` + `smaps_rollup`, macOS
  `proc_listpgrppids` + `proc_pid_rusage`), gated by `#[cfg(target_os = …)]`
  with a null-returning fallback arm.
- `ui/src/components/ProcessInspectorPanel.tsx` (+ its CSS) — the inspector
  viewer: poll loop, header, output pane, resource readout.

### Modified

- `crates/phoenix-ide/src/api/handlers.rs` — add the
  `GET /api/work-scope/:scope_key/bash/:handle_id/inspect` route and handler.
- `ui/src/contexts/ViewerSlotContext.tsx` — add the `inspect` `ViewerKind`
  variant, its URL derivation in `deriveSlot`, and an `openInspect` action.
- `ui/src/components/WorkScopePanel.tsx` — add the inspect affordance to
  `BashRow`, wired to `openInspect`.
- The right-side viewer host (where prose/diff/browser mount) — render
  `ProcessInspectorPanel` when `slot.kind === 'inspect'`.
- `ui/src/generated/` — regenerated by `./dev.py codegen` (never hand-edited).

## Cross-Spec Touchpoints

- **`specs/viewer_slot/`** — the inspector is a new `ViewerKind`. The slot's
  Allium union (`viewer_slot.allium`) enumerates `{none, prose, diff, browser}`;
  adding `inspect` there is a follow-on to that spec, not part of this spec's
  parse. This spec depends on the slot's mutex, close, reset, and URL contracts
  by path; it does not redefine them.
- **`specs/work-scope-ui/`** — the launch affordance lives on that panel's bash
  row, and the inspector reuses its `BashHandleState` vocabulary and its
  `(scope_key)` resolution. The inspector adds `exit_code` / `signal_number` to
  the per-handle projection the inventory deliberately omits.
- **`specs/bash/`** — the output delta is the ring read (REQ-BASH-004) and the
  terminal fields are the tombstone shape (REQ-BASH-006). The inspector wraps
  these reads; it does not change ring or tombstone semantics.

No SSE wire variant is added, so the SSE cross-spec whitelist
(`specs/sse_wire/`, `AUTHORING.md` §7) is not engaged.

## Non-Goals / Future Work

- **Push streaming of output.** The inspector polls (REQ-PINSP-006). An SSE/push
  stream of finalized output lines is a later optimization, unwarranted for a
  view that is open only while an operator is actively watching one handle.
- **Rich metrics.** Per-thread breakdowns, I/O counters, and open file
  descriptors are out of scope; the inspector reports the core trio (CPU,
  proportional memory, process count) only (REQ-PINSP-004). The resource row is
  the natural anchor for such overlays later.
- **Non-bash resource inspection.** The inspector is bash-handle-scoped. The
  tmux server and browser session are surfaced at the roll-up level by the
  work-scope panel (`specs/work-scope-ui/` REQ-WSUI-003, REQ-WSUI-004); a
  per-resource drill-down for them is not specified here.
- **Historical / charted resource trends.** The resource readout shows the
  latest sample, not a time series; sparklines or retained history are out of
  scope.
