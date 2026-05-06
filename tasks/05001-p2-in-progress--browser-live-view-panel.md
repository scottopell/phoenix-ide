---
created: 2026-05-06
priority: p2
status: in-progress
artifact: pending
---

# browser-live-view-panel

## Plan

# Browser Live View — collaborative side panel

## Summary

Add a live view of the conversation's Chromium instance as a side panel, parallel in spirit to the tmux terminal panel: the agent can say "look at the browser side panel and try XYZ" instead of "the dev server is at localhost:..., go check it yourself."

**Scope is deliberately view-only for the MVP.** The user watches; the agent drives. No input proxying, no take-over, no multi-tab UI. Those are real follow-ups but each is its own decision (input race policy, tab model, agent-vs-user conflict resolution) and gating MVP on them kills the momentum. View-only is pure additive user value.

## Context

The single-Chromium-per-conversation infrastructure already exists (`src/tools/browser/session.rs`, `BrowserSessionManager`). It's headless. State persists across tool calls, isolated per conversation, idle-times-out at 30 min, and is killed by the conversation hard-delete cascade. The browser is currently invisible to the user.

The architectural analog is tmux integration: per-conversation persistent server, in-app panel attaches to the same server, single shared session. For tmux the hard part was the attach protocol; the equivalent here is a CDP screencast relay. CDP's `Page.startScreencast` emits JPEG frames over the existing CDP channel — no VNC, no Xvfb, no headed Chrome required, works fine in headless server environments.

User decisions, locked in:

- **Input policy:** view-only. User cannot click or type into the browser view. The agent is the sole driver. The agent's existing `browser_*` tools are unchanged.
- **Privacy:** acknowledged. Cookies, OAuth tokens, anything the agent navigates to is visible to whoever has the panel open. Same trust model as the shared tmux session.
- **Slot policy:** the browser view occupies the right-hand viewer slot, mutually exclusive with prose reader and diff viewer.
- **Auto-mount:** on first browser tool of the conversation, *if and only if the slot is empty*. If the slot already shows prose reader or diff, leave it — don't interrupt what the user is reading. After first activation, the user's slot choice is sticky; subsequent browser tool firings never clobber. A small "browser updated" indicator on the slot tab/header is acceptable but not required for MVP.
- **Multi-tab:** out of scope. The screencast follows `BrowserSession.page` (page index 0). If the agent or page opens additional targets, the panel keeps showing the canonical page. Same "main is canonical" stance the tmux spec takes.
- **Multiple human viewers:** allowed. View-only means there's no input race, so multiple browser tabs of the Phoenix UI watching the same conversation can each render the screencast. (Different from the terminal's deliberate single-attach.)

## What to do

### Backend

1. **CDP screencast subscription.** In `src/tools/browser/`, add a screencast broker that wraps `Page.startScreencast` / `screencastFrame` / `screencastFrameAck`. It owns the page-side subscription, ack'd back to CDP for flow control, and fans frames out to N WebSocket sinks. Stop the screencast when sink count drops to zero, restart on first sink. Frame format: JPEG, quality ~70, `everyNthFrame=1` as defaults — refine if perf demands. Also subscribe to `Page.frameNavigated` so the broker carries current URL alongside frames (panel header shows it).

2. **WebSocket endpoint.** New route, e.g. `GET /api/conversations/:id/browser-view`, gated by the same auth middleware as the terminal endpoint. Reuses `BrowserSession` via the existing manager — does *not* spawn a session on its own (if no browser tool has run yet, the screencast endpoint should return cleanly with a "no browser session yet" signal so the panel can show a placeholder; auto-mount logic then doesn't fire until first browser tool anyway).

3. **Frame protocol.** Binary WebSocket frames. Suggested:
   - byte 0 = 0x00 → JPEG frame: `[0x00][u32be jpeg_length][jpeg bytes]`
   - byte 0 = 0x01 → URL change: `[0x01][utf-8 url]`
   - byte 0 = 0x02 → page metadata (title, viewport size) — optional, can defer
   Pick whichever framing is simplest; symmetrical to terminal's binary protocol. Document in module doc comment.

4. **Lifecycle wiring.** Conversation hard-delete cascade already kills the session; nothing new needed there. But: when `kill_session` runs, any active screencast WSes should be closed cleanly with a status code that the frontend treats as "session ended" (not "transient disconnect, retry").

5. **Capability gap logging.** If `Page.startScreencast` ever fails or is unsupported (shouldn't happen on chromiumoxide's bundled Chromium, but defensive), log at `debug` and surface a clear error frame to the panel — don't silently fall through.

### Frontend

1. **`ui/src/components/BrowserViewPanel.tsx`.** New component, parallel structure to `TerminalPanel`. Renders incoming JPEG frames into a `<canvas>` (cheaper than `<img src=blob:...>` cycling) with the URL in a header strip. Shows "view-only" affordance — e.g. a tooltip on hover, and pointer-events disabled on the canvas so the user can't be confused into thinking clicks work. Reconnect/backoff on WS drop (mirror terminal's pattern).

2. **Slot integration in `ui/src/pages/ConversationPage.tsx`.** The right-hand viewer slot already toggles between prose reader and diff viewer. Extend to a third option: `BrowserView`. Mutually exclusive — exactly one of {prose, diff, browser, none} at a time.

3. **Auto-mount logic.** Track two pieces of state per conversation in the UI:
   - `browserHasActivated` — flips true the first time the conversation observes a `browser_*` tool execution (existing tool-event SSE stream is sufficient — no new server signal needed).
   - The current slot occupant (already exists).

   Rule: when `browserHasActivated` flips true, *if the slot is empty*, mount the browser view. Otherwise no-op. After that, the user's explicit selection wins; never auto-displace.

4. **Manual open affordance.** Whatever surface (sidebar button, header dropdown, keyboard shortcut — pick one consistent with how prose reader / diff are opened today) lets the user open the browser view explicitly. Required because the auto-mount rule is conservative; without a manual opener the user can never see the browser if they had a prose reader open at first activation.

### Spec

5. **Update `specs/browser-tool/`:**
   - New user story: "As a user, I want to watch what the agent does in the browser in real time so I can give feedback while it works."
   - New requirement (next REQ-BT-NNN): Live View Side Panel — view-only, single canonical page, auto-mount-when-slot-empty, multi-viewer allowed, screencast over WS. Cite the input/multi-tab non-goals explicitly so a future agent doesn't "fix" them without revisiting this discussion.
   - Update `executive.md` status table.

### Tests

6. Unit-test the screencast broker's fan-out: one mock CDP frame source, N sinks attach/detach in arbitrary order, all attached sinks receive every frame after their attach point, source stops when last sink detaches.
7. Smoke test the WS endpoint with a real `BrowserSession` driven by a real `browser_navigate` against `about:blank` → `data:text/html,...`. At least one frame received, URL change observed.
8. UI: a render test confirming the slot's mutex behavior — opening browser view closes prose reader and vice versa.

## Acceptance Criteria

- [ ] First time the agent runs any `browser_*` tool in a conversation: if the right-hand viewer slot is empty, the browser view appears showing the live page. If the slot is occupied (prose reader / diff), nothing visibly happens — the user's reading isn't disturbed.
- [ ] User can manually open the browser view at any time after first activation; doing so closes whatever else was in the slot.
- [ ] Browser view shows current page URL in its header, updated when the page navigates.
- [ ] Browser view canvas is read-only — clicks and key presses on it have no effect on the underlying page (no input proxying for MVP).
- [ ] Multiple Phoenix UI tabs viewing the same conversation can each see the live stream concurrently.
- [ ] WS reconnect on transient disconnect; clean close on conversation delete (no infinite reconnect loop after the session is gone).
- [ ] `specs/browser-tool/` updated with the new user story, requirement, and explicit non-goals (input, multi-tab).
- [ ] `./dev.py check` passes (clippy, fmt, tests, codegen-stale guard, task validation).
- [ ] No regression in existing `browser_*` tools — they continue to work whether or not anyone is watching.

## Explicit non-goals (worth recording so they don't accidentally creep in)

- User input into the browser view (clicks, typing, scroll, keyboard shortcuts). The canvas is a read-only mirror.
- Tab/window management UI. The agent's `Page` is the only thing shown.
- Take-over / driving handoff between agent and user.
- Concurrent agent + user input arbitration.
- Headed Chrome, VNC, Xvfb. The screencast over headless CDP is the architectural answer.

These are deferrable follow-ups, each with its own design decisions worth making deliberately rather than as a side effect.


## Progress

