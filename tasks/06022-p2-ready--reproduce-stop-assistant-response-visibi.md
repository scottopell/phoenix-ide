Investigate the user's report that stopping a later tool can lose an earlier assistant response. Permanent loss has not been reproduced. Preserve the distinction between persistence and temporary presentation.

The completed assistant response is retained in durable ToolExecuting state before tools execute. CancellingTool retains it; cooperative abort, a racing completion and the deadline backstop should checkpoint that response plus prior and cancellation results. Recovery materializes buffered rounds before resetting state. Do not infer loss solely because the messages table has not yet received that pending round.

The source investigation found a bounded presentation hypothesis: a replay ring overflow can omit the eager assistant update during a reconnect, while full initialization reads persisted messages without projecting the buffered ToolExecuting assistant. Initialization replacement can temporarily hide the response until its final checkpoint. Stopping unfinished streamed text separately discards the partial response under the existing REQ-BED-005 policy.

Reproduce with a disposable real database and provider/tool barriers: return prose plus two tools, stop during the second, inspect live state, persisted messages, reload and reopen. Cover cooperative cancellation, completion race and deadline backstop; use existing persistence failure/recovery helpers where useful. Add a replay-overflow/reconnect control and a separate partial-stream control so the two behaviors cannot be confused. Check actual wire/UI visibility before declaring a presentation defect, and change only the demonstrated gap. No new partial-text retention policy or replay framework is commissioned by this investigation.

Starting points: crates/phoenix-state-machine/src/transition.rs ToolExecuting/CancellingTool transitions; crates/phoenix-ide/src/runtime/executor.rs checkpoint and abort-deadline execution; crates/phoenix-ide/src/runtime.rs replay ring; crates/phoenix-ide/src/api/handlers.rs initialization messages; crates/phoenix-db/src/lib.rs reset_all_to_idle. Follow specs/bedrock and the SSE/recovery contracts before changing behavior.

Captured during Ember delivery after the tracking native-write fix exposed the distinction between a durable pending round and an assistant message row. That tracking fix does not establish a Stop bug or repair this presentation hypothesis.


### Separate malformed-state probe

Scanner qualification exposed a source-level observation in
`crates/phoenix-db/src/lib.rs::parse_conversation_row` (near line 15440): when
persisted `ConvState` JSON cannot deserialize, it logs a warning and returns
`ConvState::Idle`. The scanner's deliberately corrupted, oversized metadata
fixture therefore uses raw persisted-state checks rather than treating the
conversation-row parser's fallback as proof of valid state.

This corruption fixture is **not a Stop reproduction** and does not establish
permanent response loss or an existing recovery-contract violation. As a narrow
probe in this investigation, inspect the normative bedrock/persistence/recovery
contract and actual startup/checkpoint callers before deciding whether this
fallback is intended or defective. Keep that question separate from genuine
Stop barriers, replay-overflow presentation and partial-stream policy; no new
feature, recovery framework or guarantee is commissioned by the observation.
