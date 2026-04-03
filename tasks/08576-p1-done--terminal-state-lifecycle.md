---
created: 2026-02-28
priority: p1
status: done
artifact: completed
---

# Terminal State Lifecycle

## Context

Read first:
- `specs/bedrock/design.md` — "Runtime Event Loop" section (StepResult enum)
- `specs/bedrock/design.md` — Appendix A (FM-5)

FM-5: Terminal states never exited the executor loop. Loop exit was delegated to
channel-drop semantics. `ConversationHandle` lived in RuntimeManager, which never
removed it, so the channel never closed, so the loop never exited. Sub-agent executors
ran forever.

Task 573 (done) patched this for sub-agents specifically, but the fix should be
structural: the executor loop explicitly checks for terminal states and exits, rather
than relying on channel lifecycle.

## What to Do

1. **Define `StepResult` enum:**

   ```rust
   enum StepResult {
       Continue,
       Terminal(TerminalOutcome),
   }

   enum TerminalOutcome {
       Completed(String),
       Failed(String, ErrorKind),
       ContextExhausted { summary: String },
   }
   ```

2. **Add `step_result()` method to `ConvState`** that returns `StepResult::Terminal`
   for `Completed`, `Failed`, and `ContextExhausted` states, `Continue` for all others.

3. **Modify the executor event loop** to check `state.step_result()` after every
   transition. On `Terminal`:
   - Notify RuntimeManager of completion with the `TerminalOutcome`
   - Clean up channels, drop senders
   - Return from the loop (not break-and-fall-through — explicit return)

4. **Verify RuntimeManager** removes the conversation handle on terminal notification.
   The handle should not persist after the executor exits.

5. **Verify sub-agent cleanup** — sub-agent executors reach terminal via `Completed` or
   `Failed`, hit the `StepResult::Terminal` check, exit cleanly. No channel-drop
   dependence.

## Acceptance Criteria

- Executor loop has explicit `if let StepResult::Terminal(...) = ... { return; }` check
- No reliance on channel-drop for loop exit
- Sub-agent executors exit cleanly on `Completed`/`Failed`
- RuntimeManager removes conversation handle on terminal
- `./dev.py check` passes
- Existing tests still pass (this is a refactor, not a behavior change)

## Dependencies

- None (independent of 574, 575)

## Files Likely Involved

- `src/state_machine/state.rs` — StepResult, TerminalOutcome, step_result() method
- `src/runtime/executor.rs` — event loop modification
- `src/runtime/` — RuntimeManager notification
