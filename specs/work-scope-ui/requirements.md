# Work-Scope Observability Panel

## User Story

As a Phoenix user driving a conversation or chain, I need to see the live
runtime resources the agent has spawned in this work scope — backgrounded
bash processes, the tmux server, the browser session — so that I can tell at a
glance what is running right now, spot a runaway process, and understand the
side effects the agent is accumulating without reading back through the
transcript.

This panel is the human-facing complement to the agent-facing wake-contracts
feature: both read the same underlying per-`WorkScope` resource state, one for
the LLM's situational awareness and one for the operator's.

## Domain

A `WorkScope` (see `crates/phoenix-core/src/work_scope.rs`) is the durable
owner of work-affine resources: `Worktree(path)` for managed/branch
conversations, `Conversation(id)` for Direct-mode, and `Global` for the
`/new`-page resources. Bash handles, the tmux server, and the browser session
are all keyed by `WorkScope` (`specs/bash/` REQ-BASH-WS-001, `specs/terminal/`
REQ-TMUX-WS-001, `specs/browser-tool/` REQ-BROWSER-WS-001), so a single scope
key addresses every resource a conversation's continuation chain shares.

**Scope of resources surfaced:** bash handles, tmux, and browser session
ONLY. Sub-agents are excluded — they have their own dock (`SubAgentViewerDock`)
and are not work-affine resources in the same sense.

**Persistence boundary:** the panel is a read-projection over in-memory
registries. It introduces no new persistence; every value it shows already
lives in the bash handle registry, the tmux registry, or the
`BrowserSessionManager`, and is lost on Phoenix restart exactly as those
registries are.

---

## Requirements

### REQ-WSUI-001: WorkScope Inventory Projection

WHEN the system assembles a `WorkScopeInventory` for a `WorkScope`
THE SYSTEM SHALL produce a snapshot drawn from the in-memory bash handle
registry, the tmux registry, and the `BrowserSessionManager` for that scope
AND SHALL NOT read from or write to any persistent store to do so
AND SHALL include the bash, tmux, and browser sections defined in
REQ-WSUI-002, REQ-WSUI-003, and REQ-WSUI-004.

WHEN a scope has no resource of a given kind (no live tmux server, no browser
session, no bash handles)
THE SYSTEM SHALL represent that absence explicitly (an empty handle list, an
absent tmux section, an absent browser section) rather than omitting the
inventory.

**Rationale:** A single typed snapshot, sourced only from the existing
authoritative registries, keeps the panel correct-by-construction: there is no
second copy of resource state to diverge from. Explicit absence lets the UI
distinguish "scope has no browser" from "inventory failed to load."

---

### REQ-WSUI-002: Bash Handle Inventory

WHEN the inventory includes a bash handle
THE SYSTEM SHALL report, per handle: `handle_id`, `label`, a
display-simplified `cmd`, `state` (one of `running`, `kill_pending_kernel`,
`tombstoned`), `pid`, `pgid`, `started_at`, `duration_ms` (present only when
the handle is terminal), `output_bytes`, and — for a terminal handle — the
terminal outcome `exit_code` / `signal_number`.

WHEN a handle is live (status `running` or `kill_pending_kernel`)
THE SYSTEM SHALL source `pid` and `pgid` from the handle's live data; both are
absent once the handle is terminal.

THE SYSTEM SHALL report `output_bytes` in every state — the total bytes the
process has written (monotonic, partial-inclusive, never decremented by ring
eviction). It is snapshotted into the tombstone at exit, so a terminal handle
still reports its final total (`specs/bash/` REQ-BASH-004).

WHEN a handle is tombstoned
THE SYSTEM SHALL report `duration_ms`, `exit_code`, and `signal_number` from the
tombstone. `exit_code` and `signal_number` are the raw terminal outcome: a
handle completed successfully exactly when `exit_code` is `0` and no
`signal_number` is present; a non-zero `exit_code` or any `signal_number` is a
failure or kill. They are absent while the handle is live.

THE bash inventory SHALL be sourced from the `WorkScope`-keyed bash handle
registry (`specs/bash/` REQ-BASH-WS-001), covering both live and tombstoned
handles for the scope. Assembling the per-scope handle list depends on that
registry being keyed by `WorkScope`: the scope key addresses the handle set
directly, with no conversation-to-handle join.

**Rationale:** The handle's state and identity are exactly what the bash tool
already tracks. Because the registry is `WorkScope`-keyed (`specs/bash/`
REQ-BASH-WS-001), the inventory reads one scope's handles by key rather than
reconstructing the set from conversation membership. Reporting `pid`/`pgid`
makes a runaway process actionable; `output_bytes` signals an output-heavy
command before the operator opens the tail.

---

### REQ-WSUI-003: Tmux Inventory

WHEN the inventory includes a tmux section
THE SYSTEM SHALL report whether a tmux server is present for the scope and the
server's in-memory `status` (one of `not_probed`, `live`, `gone`), read from
the per-`WorkScope` tmux registry entry (`TmuxServer.status`).

THE tmux section SHALL NOT enumerate sessions or windows, because that data is
not held in memory: obtaining it requires a live `tmux ls` socket probe (a
process spawn), which the inventory must not perform on every push.

WHEN no tmux server entry exists for the scope
THE SYSTEM SHALL report the tmux section as absent.

**Rationale:** The tmux server is the scope's persistent shell environment. The
operator needs to know it exists and its liveness status; reporting the
in-memory `status` is a pure registry read. Session and window enumeration is
out of scope (see `design.md` Non-Goals / Future Work) because it would force a
socket probe — a process spawn — on every inventory assembly, including every
push.

---

### REQ-WSUI-004: Browser Session Inventory

WHEN the inventory includes a browser section
THE SYSTEM SHALL report the session `state` (one of `live`, `torn_down`) and
`idle_ms`, the elapsed milliseconds since the session's last activity.

THE wire `state` SHALL be sourced from
`BrowserSessionManager::is_active(work_scope)`: membership maps to `live`,
absence maps to `torn_down`. `idle` SHALL NOT be a wire state; it is a frontend
presentation derived from `idle_ms` (see REQ-WSUI-010).

THE SYSTEM SHALL compute `idle_ms` at assembly time as the elapsed duration
since the live `BrowserSession`'s last activity (a monotonic `Instant`). The
inventory SHALL NOT report a wall-clock last-activity timestamp, because the
source has no absolute-clock value.

WHEN no browser session exists for the scope
THE SYSTEM SHALL report the browser section as absent (equivalently, state
`torn_down`).

**Rationale:** Browser liveness is already an authoritative
`BrowserSessionManager` query; the panel reuses it rather than inferring
liveness from message history. The session's activity marker is a monotonic
`Instant` with no absolute time, so the wire exposes a relative `idle_ms`
(consistent with the other `*_ms` fields) instead of a wall-clock timestamp.
Whether a `live` session reads as "idle" is a presentation decision the
frontend makes from `idle_ms`, not a server-side state.

---

### REQ-WSUI-005: Sub-Agents Excluded

THE WorkScopeInventory SHALL NOT include sub-agent conversations or their
state.

**Rationale:** Sub-agents are surfaced by `SubAgentViewerDock` and are a
distinct concern. Listing them here would create a parallel, divergent view of
sub-agent state and clutter a panel whose job is runtime side-effects.

---

### REQ-WSUI-006: Inventory Pull Endpoint

WHEN a client issues `GET /api/work-scope/:scope_key/inventory`
THE SYSTEM SHALL return `Json<WorkScopeInventory>` for the `WorkScope` whose
`stable_key()` equals `:scope_key`.

THE endpoint SHALL follow the existing `get_conversation` handler shape (path
parameter, `State(AppState)`, `Json<…>` response).

**Rationale:** The conversation page needs the inventory at first paint, before
any push event has fired; the chain page needs it as its only data source
(REQ-WSUI-009). A plain JSON GET keyed by `stable_key()` serves both and is
also the natural surface for any future non-UI client (REQ-WSUI-011).

---

### REQ-WSUI-007: Inventory Push Event

WHEN a bash handle in a scope is spawned, transitions to a terminal state, or
is killed, OR the scope's tmux registry entry changes — the entry is first
materialized (the first `ensure_live` for the scope, e.g. when the
conversation's terminal panel attaches), its `ServerStatus` transitions on a
later `ensure_live` (`live`→`gone` and back), or it is removed by the cleanup
cascade — OR a browser session for the scope crosses a liveness edge (up or
down)
THE SYSTEM SHALL emit a `WorkScopeUpdate` SSE event carrying a `sequence_id`
and the full refreshed `WorkScopeInventory` for that scope.

THE first-materialization emission SHALL carry the tmux entry's status as
settled by the probe/spawn (`live` or `gone`) — never the transient
`not_probed` insertion state — and SHALL fire exactly once, not once at
`not_probed` and again at the settled status.

THE `WorkScopeUpdate` event SHALL be emitted only on an actual state change,
not on a read or a probe that leaves state unchanged.

THE `WorkScopeUpdate` event SHALL carry a complete inventory snapshot, not a
delta.

**Rationale:** A resource state change must reach the open panel without a
poll. Inventory payloads are small (a handful of handles plus two small
sections), so a full snapshot per change is simpler and removes any
delta-application bug class — there is no partial state to reconcile on the
client. The tmux trigger closes a coverage gap: opening a conversation's
terminal panel materializes a tmux entry (the terminal runs `tmux attach`),
and without a tmux-side emission that entry would never reach the panel — the
bash and browser triggers alone leave a terminal-only scope showing its
on-mount snapshot indefinitely.

---

### REQ-WSUI-008: Push Event Routing

WHEN a `WorkScopeUpdate` is emitted for a `WorkScope`
THE SYSTEM SHALL deliver it to the single non-terminal conversation that
resolves to that scope.

**Rationale:** `specs/projects/` REQ-PROJ-025 (`OneBranchOneActiveWorktree`)
guarantees at most one non-terminal conversation per `WorkScope`, so a single
target is well-defined; there is no fan-out ambiguity. The routing reuses the
conversation→scope resolution the browser lifecycle bridge already performs
(see `design.md`).

---

### REQ-WSUI-009: Chain Page Single-Scope Query

WHEN the chain page renders the work-scope panel
THE SYSTEM SHALL query the one `scope_key` for the chain root and render a
standalone right-adjacent dock that shares the per-resource row vocabulary of
the conversation page's section (REQ-WSUI-010)
AND SHALL NOT aggregate per-member inventories.

**Rationale:** Because resources are `WorkScope`-keyed and a chain's members
share one scope, a single inventory query is complete. A hypothetical
conversation-keyed design would force the chain page to fan out one query per
member and merge the results, with all the divergence risk that implies; the
`WorkScope` key collapses that to one read. The chain page has no left
file-explorer panel to host a section, so it uses a standalone collapsible
dock; both surfaces render the same resource rows from shared code.

---

### REQ-WSUI-010: Conversation Page Section

WHILE the conversation page is shown on a desktop viewport
THE SYSTEM SHALL present the work scope as a section in the left file-explorer
panel, stacked with the Files, MCP, Skills, and Tasks sections, always present
whenever the conversation has a work scope so it is auto-visible without
opening a separate dock.

THE section SHALL carry its own collapse state and a live-count badge of
running resources, and when expanded SHALL show per-resource rows with inline
status glyphs, label, and elapsed time, with the bash ring-buffer tail
available on demand.

THE per-resource status glyph SHALL distinguish liveness from outcome. A
running or otherwise live resource (a running bash handle, a reachable tmux
server, a live browser session) SHALL read as a live indicator — a filled dot
meaning "alive" — never as a success check. A terminal bash handle SHALL read
as its exit outcome: a success check WHEN it exited `0` with no terminating
signal, and a failure mark WHEN it exited non-zero or was killed by a signal.
The check therefore denotes a successful completion, never an in-progress
resource.

WHEN the left file-explorer panel is itself collapsed to its badge rail
THE SYSTEM SHALL show a Work scope badge in that rail carrying the live count
of running resources, and clicking it SHALL expand the panel like the other
rail badges.

THE section SHALL update from the `WorkScopeUpdate` SSE event without churning
the rest of the conversation view.

WHERE the browser section reports `state` `live` with an `idle_ms` past a
frontend-chosen threshold
THE section MAY present the browser row as "idle" — a purely client-side
rendering derived from `idle_ms`, distinct from the wire `state` (REQ-WSUI-004).

**Rationale:** The right side of the layout is reserved for the meta viewer
(prose/diff/browser); the work scope is an always-present resource view, so it
belongs in the persistent left panel beside the other resource sections rather
than in a separate right dock. Per the UI Design Philosophy (information
density, inline status, progressive disclosure): the rail's badge answers "is
anything running?" at a glance; the expanded rows answer "what, and for how
long?"; the tail is one disclosure deeper for "what is it doing?" Field-level
render isolation keeps a resource change from re-rendering the transcript. The
"idle" presentation lives in the section because it is a display threshold over
`idle_ms`, not authoritative session state.

---

### REQ-WSUI-011: CLI Client Not a Visualization Surface

THE `phoenix-client.py` CLI SHALL NOT be required to render the work-scope
inventory.

**Rationale:** `phoenix-client.py` is a single-file text-stdout client
(`specs/simple_client/`) with no rich-UI surface. A future `work-scope` CLI
subcommand could hit the same JSON endpoint (REQ-WSUI-006) for a text dump, but
that is out of scope here and not a requirement (see `design.md` Non-Goals).
