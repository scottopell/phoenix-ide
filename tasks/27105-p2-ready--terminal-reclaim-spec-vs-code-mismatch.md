<!--
ID 27105 chosen as the next free slot above 27104.
Created without `taskmd new` since the binary isn't installed in this
env; run `./dev.py tasks fix` if reallocation needed.
-->

# Resolve terminal reclaim contradiction: spec says reject 409, code reclaims silently, panel-spec assumes the rejection

## Problem

Three artifacts disagree about what should happen when a user opens the
same conversation's terminal in a second browser tab:

1. **`specs/terminal/`** REQ-TERM-001 + REQ-TERM-003: "WHEN the terminal
   is already active for that conversation / THE SYSTEM SHALL reject the
   new WebSocket connection with HTTP 409 / AND NOT spawn a second shell."
   Status table marks both ✅ Done.

2. **`specs/terminal-panel/`** REQ-TPANEL-008 ("Don't Silently Take Over
   a Terminal I Have Open Elsewhere"): assumes the backend rejects with
   409, then asks the frontend to distinguish that close code and offer
   an explicit "Reclaim this terminal" button. Spec text: "THE SYSTEM
   SHALL NOT auto-reclaim. Silently kicking my other tab without consent
   is unacceptable." Status: ❌ Not Started.

3. **`crates/phoenix-ide/src/terminal/ws.rs`** (`acquire_handle` →
   `reclaim` at `:91-103,247-314,366-413`): the second tab silently
   detaches the first via the `attach_permit` semaphore and takes over
   the existing PTY. There is no 409 path on the WebSocket — reclaim is
   the default. The single-occupancy invariant is preserved (only one
   relay attached at a time), but the *user-facing* outcome of "your
   first tab just died with no warning" is exactly what REQ-TPANEL-008
   says SHALL NOT happen.

## Two viable directions

### A. Update specs to match silent reclaim (no code change)

Code is authoritative; the "follow me across tabs" model is closer to
how Slack / iCloud behave and is arguably the better UX once the kicked
tab gets some breadcrumb. Concretely:

- Rewrite `specs/terminal/` REQ-TERM-001 + REQ-TERM-003 to drop the 409
  language and document reclaim-by-default as the contract. Update the
  executive status table accordingly.
- Rewrite `specs/terminal-panel/` REQ-TPANEL-008 to describe the chosen
  UX. Minimum viable: a brief in-app toast on the new attacher ("This
  terminal was moved here from another tab") and a one-line dead-state
  hint on the kicked tab if it can detect the reclaim ("This terminal
  moved to another tab"). The dead-state hint is the only piece that
  needs frontend code.
- Update `specs/terminal-panel/design.md` Open Questions block to record
  the resolution.

### B. Add explicit reject + opt-in reclaim (full feature, multi-PR)

Spec is authoritative; the silent take-over is the bug REQ-TPANEL-008 was
written to prevent. Concretely:

- Backend stops auto-reclaiming and instead rejects the second WebSocket
  upgrade with HTTP 409 + a distinguishable close code (e.g. 4409 in the
  application range, with reason text `"terminal_in_use"`).
- Add a new endpoint (`DELETE /api/conversations/:id/terminal` or
  `POST /api/conversations/:id/terminal/reclaim`) that revokes the
  existing session via the existing `attach_permit` machinery, then
  returns success so the client can re-attempt the WebSocket upgrade.
- Frontend `TerminalPanel.tsx` close handler distinguishes the new close
  code; renders a "Reclaim this terminal" action that calls the new
  endpoint and re-attempts the connection on success. The other tab
  observes its existing relay being kicked (already a code path the
  reclaim plumbing handles today) and renders a clearer dead-state hint.

Either path needs a single decision then a coordinated edit across the
two specs and (for B) a coordinated implementation.

## Discovery context

Surfaced 2026-05-10 while attempting a "frontend-only slice" of
REQ-TPANEL-008 (just distinguish the WS close path so duplicate-tab
failures stop reading as a generic "Connection error"). The slice turned
out to be impossible because there is no 409 path for the frontend to
distinguish today — every duplicate-tab attempt succeeds silently on the
backend and just kicks the previous relay. See PR #64 conversation for
the trail.

## Acceptance

- A choice between A and B is recorded in `specs/terminal-panel/design.md`
  Open Questions block.
- All three artifacts (the two specs and the implementation) are
  consistent. `./dev.py check` passes including the `spec anchors` lane.
- REQ-TPANEL-008 status row in `specs/terminal-panel/executive.md` no
  longer reads ❌ Not Started — it either becomes ✅ Complete (path A) or
  it has a tracked implementation slice (path B).
