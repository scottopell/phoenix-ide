Turn-end steering drain causes brief Idle state-change notify before the inline drain emits state-change-to-LlmRequesting. UI may briefly render the conversation as Idle between the original transition (X -> Idle) and the synchronous drain (Idle -> LlmRequesting). Cosmetic only — no behavior bug.

Fix: in apply_transition_result, detect when a drain is about to fire (queue non-empty and entering Idle hook) and suppress the intermediate state-change SSE notify from the original transition effects. Drain emits its own state-change notify when it transitions to LlmRequesting.

Touch: crates/phoenix-ide/src/runtime/executor.rs apply_transition_result and run_effects_with_inline_drain.
