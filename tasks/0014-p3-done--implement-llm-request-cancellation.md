---
created: 2026-01-30
priority: p3
status: done
---

# Implement LLM Request Cancellation

## Summary

Apply the same spawned-task cancellation pattern to LLM requests so they can be aborted immediately.

## Context

We implemented immediate tool cancellation using spawned tasks with CancellationToken. However, LLM requests are still awaited inline in `make_llm_request_event()`. When user cancels during LlmRequesting, we transition to CancellingLlm but the HTTP request continues until it completes.

## Acceptance Criteria

- [x] LLM requests spawn as background tasks like tools
- [x] Effect::AbortLlm (new) triggers cancellation token
- [x] HTTP request is actually aborted (tokio::select! drops the future)
- [x] Add property test for LlmRequesting + UserCancel -> CancellingLlm + AbortLlm

## Implementation

1. Added `Effect::AbortLlm` to effect enum
2. Updated state machine: `LlmRequesting + UserCancel -> CancellingLlm` now emits `AbortLlm` effect
3. Added `Event::LlmAborted` for when LLM request is cancelled
4. Updated executor:
   - Made `llm_client` an `Arc<L>` to share with spawned tasks
   - Added `llm_cancel_token` field
   - `Effect::RequestLlm` now spawns background task with `tokio::select!` racing request vs cancellation
   - `Effect::AbortLlm` triggers the cancellation token
5. Added property tests:
   - `prop_llm_cancel_goes_to_cancelling`: LlmRequesting + UserCancel -> CancellingLlm + AbortLlm
   - `prop_cancelling_llm_plus_aborted_goes_idle`: CancellingLlm + LlmAborted -> Idle
6. Updated `test_cancel_during_llm_request` to verify fast cancellation

## Notes

Lower priority than tool cancellation since LLM requests are typically faster than long-running tools, but still valuable for stuck requests or rate limiting scenarios.
