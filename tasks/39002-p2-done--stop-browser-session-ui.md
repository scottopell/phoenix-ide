# Stop Browser Session UI

## Problem

A browser tool invocation can create a long-lived headless Chromium session even when the user redirects the agent away from browser-based work. Today the UI lets the user close the browser *viewer*, but not terminate the underlying browser session. The stale session remains live in the work scope, keeps the `◍ Browser` affordance around, and can reappear later via viewer restoration or work-scope browsing.

## Goal

Add an explicit user-facing **Stop browser** action that terminates the active browser session for the conversation/work scope. This is distinct from closing the viewer: closing hides the panel; stopping kills the underlying browser session.

## Proposed behavior

- Add a backend endpoint that terminates a browser session by work scope, e.g. `DELETE /api/work-scope/:scope_key/browser-session`.
  - Parse `scope_key` with `WorkScope::from_stable_key`.
  - Call the existing `BrowserSessionManager::kill_session(&work_scope)` primitive.
  - Preserve existing lifecycle behavior: a present session emits the destroy edge, which drives `browser_session_active = false` and work-scope inventory updates.
  - No-op success when no session exists, unless existing API conventions strongly prefer 404.
- Expose a typed frontend API helper for the endpoint.
- Add a **Stop browser** control in the live browser viewer chrome.
  - Keep the existing `×` as “close view only”.
  - Add a separate destructive/explicit action with tooltip such as “Terminate the agent’s browser session for this work scope”.
  - On success, the normal SSE falling edge should close/hide the browser viewer and remove the live browser launcher.
  - Handle request errors with existing UI error patterns rather than silently failing.
- Add the same stop affordance to the work-scope browser row.
  - `WorkScopeSection` / `WorkScopePanel` currently show a live browser row with `open →`; add a nearby `stop` action for `state === 'live'`.
  - The action should work from both the conversation side panel and the chain/work-scope dock, because both surfaces are keyed by `scopeKey`.
  - After stop, the row should update through the existing work-scope inventory refresh/SSE path; local optimistic removal is optional but must not diverge from server truth.

## Spec updates

Keep spec edits bounded to the new behavior this task implements:

- Add the user-facing browser-session termination rule to the currently authoritative browser/viewer/work-scope specs.
- Clarify that stopping the browser is session lifecycle control, not page input or browser handoff.
- Do **not** perform a broad spEARS v2 migration in this task; that belongs in the follow-up `spearsv2` migration task.

## Validation

- Backend tests:
  - Endpoint rejects malformed work-scope keys.
  - Endpoint calls `kill_session` for valid keys.
  - Killing an absent session is safe and does not emit a false lifecycle edge, matching `BrowserSessionManager::kill_session` semantics.
- Frontend tests:
  - Browser viewer renders separate close-view and stop-session controls.
  - Clicking stop calls the new API helper and handles failure visibly.
  - Live work-scope browser row renders both `open →` and `stop`; torn-down rows do not render stop.
  - Work-scope stop works in the `scopeKey`-driven surface, not only when a conversation viewer is mounted.
- Run `./dev.py check`.

## Non-goals

- Do not add an agent-facing `browser_kill` / `browser_close_session` tool in this task.
- Do not make the browser view interactive; the canvas remains view-only.
- Do not overload the existing viewer `×` to kill the session.
- Do not remove or redesign browser auto-open heuristics here; this task only adds explicit cleanup control.
