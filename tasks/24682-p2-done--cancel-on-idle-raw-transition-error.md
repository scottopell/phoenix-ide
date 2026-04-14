---
created: 2026-04-14
priority: p2
status: done
artifact: src/api/handlers.rs
---

# `POST /cancel` on an idle conversation surfaces raw Rust Debug error

## Resolution

**Three** complementary fixes — one at the API boundary, one at the SSE
boundary, and one at the source in `TransitionError` itself — so no
path can leak `Debug` strings to users.

### 1. `cancel_conversation` handler is a no-op on idle/terminal

`src/api/handlers.rs::cancel_conversation` now reads the conversation's
state from the DB before dispatching `Event::UserCancel`. If the state
is `Idle` or any terminal variant, it returns `{"ok": true, "no_op":
true}` immediately without touching the runtime. No SSE event, no state
machine round-trip.

`CancelResponse` was extended with a new `no_op: bool` field
(`#[serde(skip_serializing_if = "std::ops::Not::not")]` so the existing
`{"ok": true}` shape is preserved when the cancel actually did something).

Verified live against a running Phoenix:

```
$ curl -X POST http://localhost:8033/api/conversations/<idle>/cancel
{"ok":true,"no_op":true}
```

### 2. Typed `UserFacingError` at the SSE boundary

New module `src/runtime/user_facing_error.rs` defines a
`UserFacingError` struct with constrained construction:

- `internal()` — generic "Unexpected error" + "check the logs"
- `retryable(title, detail)`
- `fatal(title, detail)`
- `with_action(action)` — generic internal failure tagged with an action
- `from_transition_error(&TransitionError)` — exhaustive match over every
  variant, mapping each to a humanised payload. `InvalidTransition`
  always maps to `internal()` so its `Debug` payload cannot reach the
  user. The match has no wildcard arm, so adding a new `TransitionError`
  variant fails the build instead of falling through to a generic
  message.

`SseEvent::Error` now carries `UserFacingError` instead of `String`. The
SSE serialization layer emits both the legacy `message` field (so
existing UI banners keep working) and a typed `error` field for future
kind-aware UI affordances.

All four `SseEvent::Error` send sites in `executor.rs` were updated to
construct typed payloads:

- Resume failure → `UserFacingError::with_action("resume the LLM request")`
- Transition error → `from_transition_error(&e)`
- Task approval failure → `UserFacingError::retryable("Task approval failed", ...)`
- The redundant double-broadcast at the outer event loop was removed —
  `process_event` already broadcasts at the source.

### 3. `TransitionError::InvalidTransition` carries structured discriminators

The `UserFacingError` shim above closed the SSE leak, but the underlying
payload was still `InvalidTransition(String)` holding
`format!("No transition from {state:?} with event {event:?}")`. Any
future code path that Display-formatted the error somewhere
user-visible would reintroduce the same leak. That's a loose thread in
AGENTS.md terms — the type system should refuse the Debug dump
structurally, not rely on every downstream consumer to remember to
route through the shim.

Fix:

- `TransitionError::InvalidTransition` now carries two
  `&'static str` fields (`state`, `event`) sourced from
  `ConvState::variant_name` and `Event::variant_name`. Its
  `thiserror`-generated `Display` impl produces
  `"Invalid transition: no arm for state=Idle event=UserCancel"` —
  safe to show anywhere.
- `ConvState::variant_name` and `Event::variant_name` are the single
  source of truth for variant discriminators. The inline `state_name`
  helper in `executor.rs::apply_transition_result` was deleted in
  favour of the method, and two `std::mem::discriminant(..)` tracing
  calls were upgraded to use `variant_name()` for readable log output.
- The construction site at `transition.rs:1421` now passes
  `state.variant_name()` and `event.variant_name()` instead of
  `format!("{state:?} ... {event:?}")`. No Debug format survives.

After the refactor, the `TransitionError` type itself refuses to carry
payload data — the only way to populate `InvalidTransition` is with
static strings that have been manually picked from a finite set.
Adding a new `ConvState` or `Event` variant fails to compile the
corresponding `variant_name` match arm, so drift isn't possible.

### Tests

- `internal_variant_does_not_expose_debug_format` — now proves BOTH
  sides:
  - User-facing: `from_transition_error(&InvalidTransition {..})` maps
    to `internal()`, which contains neither `"UserCancel"`, `"Idle"`,
    nor `{`.
  - Operator-facing: `err.to_string()` (via `thiserror` Display)
    produces `"state=Idle event=UserCancel"` without any payload dump
    and without any stray braces.
- `agent_busy_is_retryable`, `context_exhausted_is_fatal` — sanity
  checks on the variant kinds.

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

- The authoritative banner text above was captured from the live
  DOM via `agent-browser eval` on the red toast element at repro
  time. Earlier ad-hoc notes from the UI review had transcribed it
  as *"No transitions from this state with event 'send' [state:
  Ready]"* — that was a misquote; the text shown in this task is
  what the user actually sees.
- Related but distinct: task `08517-p3-done` investigated cancellation
  state transitions at the state-machine level. This bug is about the
  *edge-of-race* path (cancel races with natural state transition to
  idle) and the UX around it, not the transition table itself.
