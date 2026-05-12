When the LLM itself rejects with a `context_length_exceeded`-class error (terminal), the state machine routes the event through the generic non-retryable LlmError path in `transition.rs:1224-1250` and lands the conv in `ConvState::Error { error_kind: ContextExhausted }`. The `/api/conversations/:id/continue` precondition in `db.rs:724` requires `ConvState::ContextExhausted { summary }` specifically — different variant — so the only way users can recover from a backend-rejected context overflow is to manually edit the DB.

Two paths trigger context exhaustion today:
1. Phoenix-internal: usage tracking sees >= 90% of declared context_window → `should_trigger_continuation` returns true → `handle_context_exhaustion` → routes to `ConvState::ContextExhausted` (works).
2. Backend-external: codex returns terminal `context_length_exceeded` (e.g. when Phoenix-declared context is wrong, or backend cap tightens, or large attachment lands mid-turn) → `LlmErrorKind::ContextWindowExceeded` → `LlmOutcome::TokenBudgetExceeded` → `Event::LlmError { error_kind: ContextExhausted }` → generic error path → `ConvState::Error` (broken).

Repro: send a message large enough to bust real backend ctx in one shot. Conv lands in Error state with `error_kind: context_exhausted`. POST /continue returns 409 "parent not context-exhausted". User is stuck unless they delete the conv or someone with DB access nudges the state.

Fix: in `handle_core_error_retry` (or a new arm), special-case `error_kind == ContextExhausted` to route directly to `ConvState::ContextExhausted { summary: message }` instead of `ConvState::Error { error_kind: ContextExhausted }`. Same precondition is then satisfied by the natural flow.

Tests: add a transition test that drives `LlmRequesting → LlmError { ContextExhausted, retryable=false }` and asserts the result is `ConvState::ContextExhausted`, not `ConvState::Error`. Also one e2e test that sends a too-big request and asserts `/continue` returns 200.

Discovered 2026-05-11 while validating the codex 272K context-window fix (sibling of 60007). Workaround for that specific conv: manual SQL UPDATE on `state` column.
