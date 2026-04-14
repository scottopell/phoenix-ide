---
created: 2026-04-14
priority: p2
status: ready
artifact: src/api/handlers.rs
---

# `POST /cancel` on an idle conversation surfaces raw Rust Debug error

## Summary

Cancelling a conversation that's already idle produces a red SSE error
toast with the raw `Debug` rendering of the Rust state machine's
`InvalidTransition` error. The HTTP call returns `200 {"ok":true}`,
but the UI then shows:

> **Invalid transition: No transition from Idle with event UserCancel { reason: None }**

Three distinct problems wrapped in one observation:

1. **The API disagrees with itself.** `POST /api/conversations/:id/cancel`
   returns `{"ok": true}` even when the state machine rejects the
   `UserCancel` transition. The client thinks the cancel succeeded;
   the user sees an error banner a moment later via SSE.
2. **Raw Rust `Debug` format leaks to users.** `UserCancel { reason:
   None }` is the `#[derive(Debug)]` output of the internal event enum.
   This should never appear in a user-facing toast.
3. **Cancel-on-idle shouldn't be an error at all.** There's nothing to
   cancel — the right answer is a silent no-op, not a state-machine
   error.

## Repro

1. `./dev.py up`
2. Create a Direct-mode conversation with the `mock` provider
3. Send any short message; mock replies quickly and the state returns
   to `Idle`
4. `curl -X POST http://localhost:8033/api/conversations/<id>/cancel`
5. API returns `{"ok": true}`
6. UI shows the red toast above

Verified against `ui/src/components/...` — the toast class is
`sse-error-toast`, and the text is the literal string emitted by
`src/state_machine/transition.rs:1421-22`:

```rust
(state, event) => Err(TransitionError::InvalidTransition(format!(
    "No transition from {state:?} with event {event:?}"
)))
```

The UI I extracted in-browser:

```json
[
  { "cls": "sse-error-toast",
    "text": "Invalid transition: No transition from Idle with event UserCancel { reason: None }Dismiss" },
  { "cls": "sse-error-text",
    "text": "Invalid transition: No transition from Idle with event UserCancel { reason: None }" }
]
```

## Proposed fix

**Primary fix — make cancel-on-idle a no-op in the handler.** In the
`/cancel` handler (`src/api/handlers.rs`), check current state before
dispatching `UserCancel`:

- If state is `Idle`, `Completed`, `Failed`, or any terminal variant,
  return `{"ok": true, "no_op": true}` without dispatching.
- Otherwise dispatch `UserCancel` as today.

This removes the error path entirely for the common race (user hits
Cancel just as the agent turns idle).

**Secondary fix — never show raw `Debug` errors in user-facing toasts.**
The SSE error path currently passes `Display`/`Debug` strings directly
to the UI. At minimum, filter/replace `InvalidTransition` errors before
emitting them on the SSE error channel:

- Log them at `warn` with full Debug so they're still diagnosable
- Don't broadcast them to the UI unless the underlying transition was
  caused by a *user-intended* action that cannot proceed (and even
  then, humanize).

**Tertiary fix (documentation / typing).** Error messages that may hit
the UI should go through an explicit `UserFacingError` shim so the
type system prevents direct Debug leakage. (Matches AGENTS.md's
"correct-by-construction" rule.)

## Done when

- [ ] `POST /cancel` on an idle conversation is a no-op and emits no
      SSE error
- [ ] `Debug` formatting of internal enums never appears in a
      user-facing toast
- [ ] Regression test: cancel-after-settled produces `{"ok":true,
      "no_op":true}` and zero SSE error events
- [ ] Manual repro (mock + cancel-after-reply) shows no toast

## Notes

- The screenshot `tasks/screenshots/12-fresh-conversation.png` shows
  this banner. An earlier version of `tasks/screenshots/README.md`
  misquoted it as *"No transitions from this state with event 'send'
  [state: Ready]"*; the actual text has been captured here from the
  live DOM and is the authoritative version.
- Related but distinct: task `08517-p3-done` investigated cancellation
  state transitions at the state-machine level. This bug is about the
  *edge-of-race* path (cancel races with natural state transition to
  idle) and the UX around it, not the transition table itself.
