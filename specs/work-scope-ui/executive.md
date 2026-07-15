# Work-Scope Observability Panel — Executive Summary

## Requirements Summary

A side panel that shows, per `WorkScope`, the live runtime resources the agent
has spawned — backgrounded bash processes, the tmux server, and the browser
session — so the user can see "what is running right now" on both the
conversation page and the chain page. It is the human-facing complement to the
agent-facing wake-contracts feature; both read the same per-`WorkScope`
resource state.

The panel is a read-projection: it introduces no new persistence. A
`WorkScopeInventory` snapshot is assembled on demand from the existing in-memory
registries (bash handle registry, tmux registry, `BrowserSessionManager`).
Sub-agents are deliberately excluded — they have their own dock. The inventory
reaches the client two ways: a pull endpoint
(`GET /api/work-scope/:scope_key/inventory`) for initial load and the chain
page, and a `WorkScopeUpdate` SSE event that re-broadcasts the full snapshot
when a resource changes state. The conversation page mounts a collapsed-by-
default rail (live-count badge) that expands to per-resource rows with inline
status glyphs, labels, elapsed time, and shared CPU/proportional-memory/process-count health for live bash handles. Detailed output remains in the process inspector. The
chain page renders the identical panel against the one scope key of the chain
root.

## Technical Summary

`WorkScopeInventory` (defined in `phoenix-core::domain`) is the wire projection
over three `WorkScope`-keyed registries. The pull handler mirrors
`get_conversation`, keyed by `WorkScope::stable_key()`. The push path adds a
`WorkScopeUpdate` variant to `SseWireEvent` (full-snapshot-on-change, no
deltas) and registers it as a per-stream `ReplayRing` member so a reconnecting
client gets the latest snapshot; both are carried through the ts_rs codegen
path (`#[derive(TS)]` + `export_to ui/src/generated/`, then
`./dev.py codegen`, plus a valibot schema in `sseSchemas.ts`). Routing targets
the single non-terminal conversation in the scope — sound because
`specs/projects/` REQ-PROJ-025 (`OneBranchOneActiveWorktree`) guarantees at most
one non-terminal conversation per scope — reusing the conversation→scope
resolution the browser lifecycle bridge already performs, with scope caching on
the runtime handle as a latency optimization. On the client, a `workScope` field
is added to `ConversationAtom` with a reducer branch guarded by `applyIfNewer`
(mirroring `sse_state_change`); `useConversationView` field-level isolation keeps
inventory changes from churning the transcript. On the conversation page the
work scope is a section in the left `FileExplorerPanel`, stacked with
Files/Skills/Tasks and always present; the chain page, which has no left panel,
uses a standalone right-adjacent dock that shares the same resource rows.
The pull projection attaches one timestamped, deduplicated scope aggregate and per-handle health from the same demand-driven observation generation used by `/about` and process inspectors; lifecycle-only snapshots remain valid where native metrics are unavailable.

No Allium spec accompanies this spec: the feature is a read-projection plus a
full-snapshot push over resource state whose lifecycles are already modeled by
`specs/bash/`, `specs/terminal/`, and `specs/browser-tool/`. It introduces no
new state machine, precondition, or ordering obligation, so spEARS alone is the
correct weight (see `design.md`, "Why No Allium Spec").

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-WSUI-001:** WorkScope Inventory Projection | Proposed | Read-projection over in-memory registries; no new persistence |
| **REQ-WSUI-002:** Bash Handle Inventory | Proposed | Sourced from the `WorkScope`-keyed bash registry (`specs/bash/` REQ-BASH-WS-001) |
| **REQ-WSUI-003:** Tmux Inventory | Proposed | Server presence + in-memory `TmuxServer.status`; no session/window enumeration |
| **REQ-WSUI-004:** Browser Session Inventory | Proposed | `BrowserSessionManager::is_active` → `live`/`torn_down`; relative `idle_ms`, no wall-clock |
| **REQ-WSUI-005:** Sub-Agents Excluded | Proposed | Sub-agents own `SubAgentViewerDock` |
| **REQ-WSUI-006:** Inventory Pull Endpoint | Proposed | `GET /api/work-scope/:scope_key/inventory`; `get_conversation` shape |
| **REQ-WSUI-007:** Inventory Push Event | Proposed | `WorkScopeUpdate` `SseWireEvent`; full snapshot, no deltas |
| **REQ-WSUI-008:** Push Event Routing | Proposed | Single non-terminal conversation per scope (REQ-PROJ-025) |
| **REQ-WSUI-009:** Chain Page Active-Member Scope Query | Proposed | Active (latest) member's scope key, root fallback; standalone dock sharing the section's rows; no per-member fan-out; SSE-less dock polls while collapsed |
| **REQ-WSUI-010:** Conversation Page Section | Proposed | `WorkScopeSection` in left `FileExplorerPanel`, stacked with Files/Skills/Tasks; collapsed-rail badge; atom `workScope` field |
| **REQ-WSUI-011:** CLI Client Not a Visualization Surface | Proposed | `phoenix-client.py` text-only; CLI subcommand is future work |

**Progress:** 0 of 11 implemented.
