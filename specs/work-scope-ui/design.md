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
                                       WorkScopeSection (left panel) + WorkScopePanel dock (chain)
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
    exit_code:       Option<i32>,   // terminal outcome; present only when terminal
    signal_number:   Option<i32>,   // terminal outcome; present only when killed by a signal
    output_bytes:    u64,           // total bytes written; always present, persisted in tombstone
}

TmuxInventory {
    status:  TmuxServerStatus,           // not_probed | live | gone
}                                        // presence encoded by Option<TmuxInventory>

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
  by the presence of a `KillAttempt`) from `HandleState::Tombstoned`; `pid`
  and `pgid` come from `LiveData`; `duration_ms`, `exit_code`, and
  `signal_number` come from the `Tombstone`; `output_bytes` is the ring's
  monotonic total (`RingBuffer::output_bytes` while live, the `Tombstone`'s
  snapshotted final total once terminal).
- **Tmux** (REQ-WSUI-003): the per-`WorkScope` tmux registry. The registry
  entry (`TmuxServer`) records `work_scope`, `socket_path`, and an in-memory
  `status` (`NotProbed` | `Live` | `Gone`). The inventory carries
  `Some(TmuxInventory { status })` when an entry exists and `None` when it does
  not — presence is the `Option`, never a separate flag — pure in-memory reads.
  It does not enumerate
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

Three registries publish scope-change signals into one work-scope bridge, each
through its own lifecycle sink (`Option<UnboundedSender<…Event>>`, `None` for
tool-level tests so existing constructors are unaffected):

- The bash registry emits on spawn / transition-to-terminal / kill, and on
  cascade removal — when the terminal-transition cascade actually removes the
  scope's handle table and SIGKILLs its live process groups. The cascade emit
  fires only on the teardown path (a handle table was removed), never on the
  preserved early return (a live owner still holds the scope) or a no-entry
  cascade (nothing to remove). Without it, a scope whose only change is the
  bash teardown — no concurrent tmux/browser edge — would leave the collapsed
  work-scope badge showing the killed handles.
- The tmux registry emits on first materialization (the first `ensure_live`
  for a scope), on a later `ServerStatus` transition, and on cascade
  removal — only on an actual state change, never on a probe-noop against an
  already-`live` server. The first-materialization emit fires *after* the
  probe/spawn settles the status, so it carries the settled `live`/`gone`
  status rather than the transient `not_probed` insertion state, and it fires
  exactly once for the create→settled path (not once at `not_probed` then
  again at the transition).
- The browser manager publishes liveness edges via its
  `BrowserSessionLifecycleSink`.

The runtime feeds all three into a single `tokio::select!` loop in the
work-scope bridge; each arm yields the affected `WorkScope`, and the loop calls
one shared routing path (`broadcast_work_scope_update`) that assembles the full
inventory and sends a `WorkScopeUpdate` through the target conversation's
`SseBroadcaster::send_seq`, the same allocator every other per-conversation
event uses. Routing the three signal kinds through one path (rather than three
parallel mechanisms) keeps the assembly-and-resolve logic single-source.

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

### Conversation page section (REQ-WSUI-010)

`WorkScopeSection` is a collapsible section in the left `FileExplorerPanel`,
stacked with the Files, MCP, Skills, and Tasks sections. It mirrors the
header + own-expand-state pattern of `SkillsPanel` / `TasksPanel`: a header
button with a chevron, a summary label, and a live-count badge, with the
dense resource body below when expanded. It is always present whenever the
conversation has a `work_scope_key`, so it is auto-visible without opening a
separate dock — the right side of the layout is reserved for the meta viewer
(prose/diff/browser).

The right side hosts no work-scope surface on the conversation page;
`DesktopLayout` threads the conversation's `work_scope_key` and the live
`workScope` inventory into `FileExplorerPanel`, which renders the section and
its collapsed-rail badge.

Per the UI Design Philosophy:

- **Section header (default-expanded):** a "Work scope" label with a
  live-count badge — the number of `running` resources across bash + browser.
  Answers "is anything running?" at a glance.
- **Expanded body:** per-resource rows. Each row shows an inline status glyph,
  the resource label, and elapsed time inline. The glyph separates liveness
  from outcome: a live resource (running bash handle, reachable tmux server,
  live browser session) is a green live dot `●` ("alive"); a
  `kill_pending_kernel` handle is a yellow `⏱` (terminating); a terminal bash
  handle is its outcome — a green `✓` when it exited `0`, a red `✗` when it
  exited non-zero or was killed by a signal (the title carries the precise
  status, e.g. `exited 3` / `killed (signal 9)`); a torn-down browser is a
  muted `○`. The check thus means "completed successfully," never "running."
  The bash ring-buffer tail is a per-row on-demand disclosure, not shown by
  default.
- **Left panel collapsed:** when the whole `FileExplorerPanel` is collapsed to
  its badge rail, a Work scope badge in that rail carries the live count;
  clicking it expands the panel like the Files/Skills/Tasks badges.

```
Expanded section (in left panel)
┌──────────────────────────┐
│ ▾ Work scope          ⦿3 │
│ ● bash  npm test   1m12s │
│ ⏱ bash  build…     0m03s │
│ ✓ bash  lint      (done) │
│ ✗ bash  test      (fail) │
│ ● browser  live    8m    │
│ ● tmux  main (2 win)     │
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
re-renders only the section subscribers, not the transcript (REQ-WSUI-010).
The section's initial fetch (REQ-WSUI-006) seeds only its local state; the
atom's `workScope` remains written solely by the SSE reducer — a single-writer
contract that keeps the live push authoritative.

### Live-resource poll

The `WorkScopeUpdate` push is edge-triggered on state transitions, so fields
that drift continuously between transitions stay frozen — a live bash handle's
`output_bytes` grows as the process emits output, and a `live` browser
session's `idle_ms` advances every second with no dedicated edge. To close that
gap, the panel re-fetches the pull endpoint on a fixed cadence (~2s) while the
surface is active **and** the scope owns any live resource, merging the result
into its local displayed snapshot (last-arrival-wins).

The poll gate is "any live resource", not "a running bash handle": it is true
when any bash handle is running / `kill_pending_kernel`, OR a tmux entry exists
and is `live` or `not_probed` (a `gone` entry is terminal), OR a browser
session is `live`. The broader gate is defense in depth — it keeps the panel
refreshing for values with no dedicated emit (browser `idle_ms`) and is
belt-and-suspenders for tmux, whose entry can be created off the conversation's
own SSE channel (the terminal panel's `tmux attach`). It is self-limiting: once
nothing is live, or the surface unmounts / collapses, the poll stops, so there
are no unbounded background timers.

### Chain page (REQ-WSUI-009)

The chain page has no left `FileExplorerPanel` to host a section, so it queries
the `scope_key` of the chain's active (latest) member via the pull endpoint and
renders the standalone right-adjacent `WorkScopePanel` dock (width via
`useResizablePane`, collapsed by default). The active member is the one with
`position === "latest"`; a single-member chain has no `latest`, so the dock
falls back to the root. Both surfaces render the same per-resource rows from
shared code in `WorkScopePanel.tsx`.

The active member, not the root, is the right scope because the chain's members
do not all share one scope. Shared-worktree chains (Worktree / Branch / Work)
resolve every member to the same worktree scope, but Direct continuation chains
resolve each member to a distinct `WorkScope::Conversation(<member id>)`, so the
leaf's live resources are under its own scope, not the root's. The latest
member's scope is correct for both: it is the shared worktree scope for the
former and the leaf's own conversation scope for the latter. No per-member
aggregation is needed — one query against the active member's `WorkScope` key is
complete; a conversation-keyed fan-out across every member with a client-side
merge would add divergence risk for no gain.

Because the chain dock has no per-conversation SSE channel (it omits
`liveInventory`), its live-resource poll must stay active even while the dock is
collapsed: with no push to correct it, a resource that is live at collapse and
then exits or is reaped would otherwise leave the collapsed count badge reading
"running" indefinitely. An SSE-less surface therefore keeps polling while
collapsed (the poll still self-limits once nothing is live), whereas an
SSE-backed surface pauses its poll when collapsed and relies on the push to keep
the badge fresh.

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
- **`pid` / `pgid` / `exit_code` / `signal_number` / `duration_ms` are `Option`.**
  `pid`/`pgid` exist only while `Live`; `exit_code`/`signal_number`/`duration_ms`
  only once `Tombstoned`. Skipping the field via
  `skip_serializing_if = "Option::is_none"` renders it `undefined` on the TS
  side, which the schema treats as optional. **`output_bytes` is NOT `Option`** —
  total output is defined in every state (0 at spawn) and is persisted into the
  tombstone, so it is always present.
- **`duration_ms` is `Option<u64>`**, present only for terminal handles, the
  same shape `MessageUpdated.duration_ms` uses in `wire.rs`.

## Files to Create / Modify

### New

- `phoenix-core::domain`: `WorkScopeInventory` + nested types, with `ts_rs`
  derives. This is where Phoenix's other wire-projection/domain types live
  (e.g. `phoenix_core::domain::tool_wire`, per the layering note in
  `crates/phoenix-ide/src/api/wire.rs`), so the `tools` and `api` layers can
  depend *down* onto them.
- `ui/src/components/WorkScopePanel.tsx` (+ per-resource row, ring-tail
  disclosure) and its CSS. Exports both `WorkScopeSection` (the left-panel
  section for the conversation page) and `WorkScopePanel` (the standalone dock
  for the chain page), which share the row vocabulary.

### Modified

- `crates/phoenix-ide/src/api/wire.rs` — add the `WorkScopeUpdate`
  `SseWireEvent` variant, its `event_type()` arm, and the
  `From<SseEvent>` conversion arm.
- `crates/phoenix-ide/src/runtime.rs` — add the `SseEvent::WorkScopeUpdate`
  source variant and the scope→conversation routing for emission.
- `crates/phoenix-ide/src/api/handlers.rs` — add the
  `GET /api/work-scope/:scope_key/inventory` route and handler.
- The bash handle registry — emit on spawn / terminal / kill / cascade
  removal (teardown path only), via the same lifecycle-sink shape.
- The tmux registry — emit on entry creation / `ServerStatus` transition /
  cascade removal, via the same lifecycle-sink shape.
- `ui/src/sseSchemas.ts` — add `SseWorkScopeUpdateDataSchema`.
- `ui/src/conversation/atom.ts` — add `workScope` field, `SSEAction` case, and
  reducer branch.
- `ui/src/components/DesktopLayout.tsx` — thread the conversation's
  `work_scope_key` and live `workScope` (via `useWorkScope`) into
  `FileExplorerPanel`, which renders `WorkScopeSection`.
- `ui/src/components/FileExplorer/FileExplorerPanel.tsx` — render
  `WorkScopeSection` in the expanded section stack and a Work scope badge in
  the collapsed badge rail.
- `ui/src/pages/ChainPage.tsx` — `ChainWorkScopeDock` mounts the standalone
  `WorkScopePanel` with a `useResizablePane` width.
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
