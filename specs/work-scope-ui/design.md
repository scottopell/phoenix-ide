# Work-Scope Observability Panel — Design Document

## Architecture Overview

The panel is a read-projection over three in-memory registries — the
`WorkScope`-keyed bash handle registry, the tmux registry, and the
`BrowserSessionManager` — surfaced over two transports:

- a **pull** endpoint, `GET /api/work-scope/:scope_key/inventory`, for initial
  load and the chain page, and
- a **push** SSE variant, `WorkScopeUpdate`, that re-broadcasts the full
  inventory when a resource changes state.

```
   bash registry ─┐
   tmux registry ─┼─► assemble WorkScopeInventory(scope) ─┬─► GET …/inventory   (pull)
   BrowserSessMgr─┘                                       └─► WorkScopeUpdate    (push)
                                                                    │
                                                          per-conversation SSE
                                                                    │
                                              ConversationAtom.workScope (reducer)
                                                                    │
                                              WorkScopePanel dock (conversation + chain)
```

There is exactly one carrier of resource state at each layer: the registries
own it, `WorkScopeInventory` is the wire projection, and `ConversationAtom.
workScope` holds the client copy. No layer keeps a second, separately-updated
representation.

## Why No Allium Spec

Allium is warranted for state machines, lifecycle flows with preconditions, and
multi-step operations where ordering matters (`AGENTS.md`, "When to write an
Allium spec"). This feature has none of those of its own: the *resources* it
observes are already modeled by `specs/bash/`, `specs/terminal/`, and
`specs/browser-tool/` (the latter two with their own Allium specs), which own
the lifecycle transitions and invariants. The panel is a pure read-projection
plus a full-snapshot-on-change push — it adds no new state machine, no new
precondition, and no new ordering obligation beyond "the snapshot reflects the
registries at emit time." spEARS alone is the correct weight here; an Allium
spec would re-state the resource lifecycles it does not own.

## Data Model

### `WorkScopeInventory`

The wire projection, defined in `phoenix-core::domain` alongside the other
wire-projection/domain types. Assembled on demand; never stored.

```text
WorkScopeInventory {
    scope_key: String,              // WorkScope::stable_key()
    bash:      Vec<BashHandleInventory>,
    tmux:      Option<TmuxInventory>,
    browser:   Option<BrowserInventory>,
}

BashHandleInventory {
    handle_id:       String,        // Handle.handle_id
    label:           Option<String>,
    cmd:             String,        // display-simplified
    state:           BashHandleState,   // running | kill_pending_kernel | tombstoned
    pid:             Option<u32>,   // present while live
    pgid:            Option<i32>,   // present while live
    started_at:      DateTime<Utc>, // RFC3339 on the wire
    duration_ms:     Option<u64>,   // present only when terminal
    ring_bytes_used: Option<u64>,   // present while live
}

TmuxInventory {
    present: bool,
    status:  TmuxServerStatus,           // not_probed | live | gone
}

BrowserInventory {
    state:   BrowserSessionLiveness,     // live | torn_down
    idle_ms: u64,                        // elapsed since last activity, at assembly time
}
```

(`BrowserSessionLiveness` is the inventory's two-value liveness enum — distinct
from the existing `BrowserSessionState` SSE event, which signals up/down edges.)

`BashHandleState`, `BrowserSessionLiveness`, `TmuxServerStatus`, and the timestamp
shapes are the contract; see [Wire-Shape Notes](#wire-shape-notes) for the
conversions from the registry types, which do not all hold a wire-ready value.

### Sources, per section

- **Bash** (REQ-WSUI-002): the `WorkScope`-keyed bash handle registry
  (`specs/bash/` REQ-BASH-WS-001). For each handle: `handle_id`, `label`,
  `cmd`, and `started_at` are fields on `Handle`; `state` discriminates
  `HandleState::Live` (further split into `running` vs `kill_pending_kernel`
  by the presence of a `KillAttempt`) from `HandleState::Tombstoned`; `pid`,
  `pgid`, and `ring_bytes_used` come from `LiveData`; `duration_ms` comes from
  the `Tombstone`.
- **Tmux** (REQ-WSUI-003): the per-`WorkScope` tmux registry. The registry
  entry (`TmuxServer`) records `work_scope`, `socket_path`, and an in-memory
  `status` (`NotProbed` | `Live` | `Gone`). The inventory reports `present`
  and `status` from that entry — pure in-memory reads. It does not enumerate
  sessions or windows: that requires a `tmux ls` socket probe (a process
  spawn) the inventory must not run on every assembly (see Non-Goals / Future
  Work).
- **Browser** (REQ-WSUI-004): `BrowserSessionManager::is_active(work_scope)`
  maps to `state` (`live` on membership, `torn_down` on absence). `idle_ms` is
  computed at assembly time from the live `BrowserSession`'s last-activity
  `Instant` (`Instant::elapsed().as_millis()`); there is no wall-clock
  timestamp on the wire.

## Pull Endpoint (REQ-WSUI-006)

`GET /api/work-scope/:scope_key/inventory → Json<WorkScopeInventory>`.

The handler mirrors `get_conversation` in `api/handlers.rs`: it takes
`State(AppState)` and `Path(scope_key)`, reconstructs the lookup against the
three registries keyed by `scope_key`, assembles the inventory, and returns it
as JSON. The `:scope_key` path segment is a `WorkScope::stable_key()` value
(`worktree:<path>`, `conversation:<id>`, or `global:`); the disjoint namespaces
that `stable_key` guarantees prevent a worktree key from resolving a
conversation-scoped resource.

## Push Event (REQ-WSUI-007)

### Wire variant

A new variant on `SseWireEvent` in `api/wire.rs`:

```text
WorkScopeUpdate {
    sequence_id: i64,
    inventory:   WorkScopeInventory,
}
```

It carries the **full** inventory snapshot, not a delta (REQ-WSUI-007): the
payload is small and a full snapshot has no partial-state reconciliation bug
class. The SSE `event:` label is `work_scope_update` (the `snake_case` tag, per
the enum's `#[serde(tag = "type", rename_all = "snake_case")]`), and the
variant is added to `SseWireEvent::event_type()`.

### Codegen path (per AGENTS.md "TypeScript codegen for SSE types")

`WorkScopeInventory` and its nested types carry
`#[derive(ts_rs::TS)]` + `#[ts(export, export_to = "../../../ui/src/generated/")]`
so `cargo test` emits the matching TypeScript under `ui/src/generated/`. After
adding the variant, `./dev.py codegen` regenerates the files; `./dev.py check`
fails on a stale `git diff -- ui/src/generated/`. A valibot schema
`SseWorkScopeUpdateDataSchema` is added to `ui/src/sseSchemas.ts`, annotated
`satisfies v.GenericSchema<unknown, WireWorkScopeUpdateData>` against the
generated type, with `sequence_id: v.number()` required like every other event
schema. The inventory body may be validated structurally or carried as
`v.unknown()` and narrowed by the reducer — see the existing opaque-field
pattern in `wire.rs` and `sseSchemas.ts`.

### Emission

The bash registry emits on spawn / transition-to-terminal / kill; the browser
manager already publishes liveness edges via its `BrowserSessionLifecycleSink`.
Each emission resolves the scope's target conversation (next section) and sends
a `WorkScopeUpdate` carrying a freshly-assembled inventory through that
conversation's `SseBroadcaster::send_seq`, the same allocator every other
per-conversation event uses.

## Push Event Routing (REQ-WSUI-008)

The event targets the single non-terminal conversation that resolves to the
scope. This is well-defined because `specs/projects/` REQ-PROJ-025
(`OneBranchOneActiveWorktree`) guarantees at most one non-terminal conversation
per `WorkScope`.

The resolution reuses the mechanism the browser lifecycle bridge already
applies: enumerate live runtime handles, resolve each conversation's
`WorkScope` via `WorkScope::resolve(conversation_id, worktree_path)`, and match
against the event's scope. The browser bridge fans out to every matching
runtime (a continuation chain may have several live handles); the work-scope
panel narrows to the single non-terminal member, which REQ-PROJ-025 makes
unique.

The conversation→scope resolution is a per-event DB read per live runtime
handle. The cheap optimization is to cache the resolved `WorkScope` on the
runtime conversation handle at get-or-create time, turning the routing lookup
into an in-memory map scan; the same caching opportunity is noted for the
browser bridge. The panel's correctness does not depend on the cache — it is a
latency optimization over the same resolution.

## Frontend

### Conversation page dock (REQ-WSUI-010)

A new right-adjacent dock in `DesktopLayout`, mounted beside
`SubAgentViewerDock`, with width managed by `useResizablePane` (the same hook
the sub-agent dock and file-explorer panel use). It is desktop-only and
gated on an active slug, matching the other docks.

Per the UI Design Philosophy:

- **Collapsed (default):** a narrow rail showing a live-count badge — the
  number of `running` resources across bash + browser. Answers "is anything
  running?" without occupying width.
- **Expanded:** per-resource rows. Each row shows an inline status glyph
  (green `✓` running, yellow `+` spawning/`kill_pending_kernel`, muted glyph
  tombstoned/torn-down), the resource label, and elapsed time inline. The bash
  ring-buffer tail is a per-row on-demand disclosure, not shown by default.

```
Collapsed rail          Expanded panel
┌────┐                  ┌──────────────────────────┐
│ ⦿3 │                  │ Work scope               │
│    │                  │ ✓ bash  npm test   1m12s │
│    │                  │ + bash  build…     0m03s │
│    │                  │ · bash  lint      (done) │
│    │                  │ ✓ browser  live    8m    │
│ [▶]│                  │ ✓ tmux  main (2 win)     │
└────┘                  │                      [◀] │
                        └──────────────────────────┘
```

### Atom integration (REQ-WSUI-010)

Add a `workScope` field to `ConversationAtom` (`ui/src/conversation/atom.ts`)
holding the latest `WorkScopeInventory` (or `null` before first load). Add an
`SSEAction` case for the new event and a reducer branch guarded by
`applyIfNewer` — mirroring the `sse_state_change` / `sse_browser_session_state`
cases — so a replayed snapshot after reconnect cannot regress a newer one.
Initial load seeds `workScope` from the pull endpoint (REQ-WSUI-006); the SSE
event replaces it wholesale on each change (full-snapshot semantics,
REQ-WSUI-007).

`useConversationView`'s field-level isolation means a `workScope` change
re-renders only the panel subscribers, not the transcript (REQ-WSUI-010).

### Chain page (REQ-WSUI-009)

The chain page queries the one `scope_key` for the chain root via the pull
endpoint and renders the identical `WorkScopePanel` component. No per-member
aggregation: because resources are `WorkScope`-keyed and the chain's members
share one scope, one query is complete. A conversation-keyed design would
instead require one query per member and a client-side merge — the fan-out the
`WorkScope` key eliminates.

## Wire-Shape Notes

- **Timestamps.** `Handle.started_at` is a `SystemTime`; the inventory reports
  it as `DateTime<Utc>`, which serde renders as an RFC3339 string on the wire
  (not `i64`) — the client parses it once, matching the `state_updated_at`
  convention.
- **Browser activity is relative, not wall-clock.** `BrowserSession`'s
  last-activity marker is a `std::time::Instant`, which has no absolute-clock
  value. The inventory exposes browser idle as `idle_ms`, computed at assembly
  time from the elapsed duration since that `Instant` — a `u64` consistent with
  the other `*_ms` fields. There is deliberately no wall-clock `last_activity`
  on the wire; reconstructing one from a monotonic clock would invent precision
  the source does not have.
- **`ring_bytes_used` / `pid` / `pgid` are `Option`.** They exist only while
  the handle is `Live`; a tombstoned handle reports them absent. Skipping the
  field via `skip_serializing_if = "Option::is_none"` renders it `undefined`
  on the TS side, which the schema treats as optional.
- **`duration_ms` is `Option<u64>`**, present only for terminal handles, the
  same shape `MessageUpdated.duration_ms` uses in `wire.rs`.

## Files to Create / Modify

### New

- `phoenix-core::domain`: `WorkScopeInventory` + nested types, with `ts_rs`
  derives. This is where Phoenix's other wire-projection/domain types live
  (e.g. `phoenix_core::domain::tool_wire`, per the layering note in
  `crates/phoenix-ide/src/api/wire.rs`), so the `tools` and `api` layers can
  depend *down* onto them.
- `ui/src/components/WorkScopePanel.tsx` (+ collapsed rail, per-resource row,
  ring-tail disclosure) and its CSS.

### Modified

- `crates/phoenix-ide/src/api/wire.rs` — add the `WorkScopeUpdate`
  `SseWireEvent` variant, its `event_type()` arm, and the
  `From<SseEvent>` conversion arm.
- `crates/phoenix-ide/src/runtime.rs` — add the `SseEvent::WorkScopeUpdate`
  source variant and the scope→conversation routing for emission.
- `crates/phoenix-ide/src/api/handlers.rs` — add the
  `GET /api/work-scope/:scope_key/inventory` route and handler.
- The bash handle registry — emit on spawn / terminal / kill.
- `ui/src/sseSchemas.ts` — add `SseWorkScopeUpdateDataSchema`.
- `ui/src/conversation/atom.ts` — add `workScope` field, `SSEAction` case, and
  reducer branch.
- `ui/src/components/DesktopLayout.tsx` — mount the dock beside
  `SubAgentViewerDock` with a `useResizablePane` width.
- `ui/src/generated/` — regenerated by `./dev.py codegen` (never hand-edited).

## Cross-Spec Touchpoints

Adding an `SseWireEvent` variant requires updating the SSE-variant enumeration
in `specs/sse_wire/sse_wire.allium` (the `EphemeralEventAppendedToReplayRing`
rule's cross-spec checklist), per `specs/AUTHORING.md` §7. `WorkScopeUpdate`
is included in the per-stream `ReplayRing`, mirroring `BrowserSessionState`:
because each event carries a full inventory snapshot, latest-wins replay is
correct, and a reconnecting client receives the most recent snapshot without
waiting for the next resource-state change. The new variant must be registered
in `specs/sse_wire/` as a replay-ring member in the same change.

## Non-Goals / Future Work

- **CLI visualization.** `phoenix-client.py` does not render the inventory
  (REQ-WSUI-011). A future `work-scope` subcommand could dump the pull
  endpoint's JSON as text, but it is not specified here.
- **Process observability overlays.** The per-resource row is the natural
  anchor for later syscall/eBPF process-observability overlays and security
  guardrails; those are explicitly out of scope for this spec.
- **Delta push.** Inventory snapshots are full (REQ-WSUI-007); a delta protocol
  is not specified and is unwarranted at the observed payload sizes.
