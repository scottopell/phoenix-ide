The executor outcome plumbing validates outcomes by current state SHAPE only, not by identity/epoch/liveness. Three concrete bugs in this family; the durable fix is to epoch-tag outcomes and turn sender-drop into a typed failure outcome.

## H2 — retry timers carry no epoch, never cancelled (HIGH, also a token-cost bug)
Effect::ScheduleRetry spawns a detached sleep -> EffectOutcome::RetryTimeout { attempt } (executor.rs ~1632). Attempt numbers are reused across turns and the only guard is `attempt == retry_attempt`. Cancel-then-resend: stale timer from the old turn passes the guard and fires a second concurrent RequestLlm. dispatch_llm_request has no in-flight check and overwrites llm_task_handle -> two concurrent LLM requests (double token cost); if the duplicate wins the race the wrong response persists. Same hole on the AwaitingContinuation retry path.
Fix: tag ScheduleRetry/RetryTimeout with a per-turn generation, or store an AbortHandle and cancel on any transition out of the scheduling state.

## M1 — sub-agent timeout double-delivers (MEDIUM)
handle_sub_agent_timeout (executor.rs ~1115) injects synthetic TimedOut for each pending agent AND sends a cancel; the cancel produces a second real SubAgentResult that lingers in the buffer and can surface a spurious user-visible SseEvent::Error in a later round. A real success already in flight is overwritten by TimedOut.
Fix: make timeout follow the cancellation protocol (enter CancellingSubAgents, let real/synthesized results drain) instead of racing.

## M2 — sub-agent UserCancel during ToolExecuting skips AbortTool (MEDIUM)
transition.rs ~2507 matches SubAgentState::Core + UserCancel and goes straight to Failed + NotifyParent with no Effect::AbortTool. A Work-mode sub-agent bash/patch keeps mutating the shared worktree after the parent was told it stopped. Parent path correctly routes through CancellingTool/AbortTool.
Fix: route sub-agent cancellation from ToolExecuting through the same CancellingTool/AbortTool sequence.

## M3 — panicked tool task wedges the conversation forever (MEDIUM)
Outcome forwarders are `if let Ok(x) = rx.await` (executor.rs ~2388 tool, ~2186 LLM). If the spawned task panics the oneshot sender drops and nothing is delivered. For tools: ToolExecuting never gets ToolComplete; user cancel moves to CancellingTool which waits forever and rejects all input — unrecoverable until restart. No tool deadline exists.
Fix: map Err(RecvError) -> ToolExecOutcome::Failed (resp. LlmOutcome::NetworkError) so sender-drop is structurally impossible to lose.

Found in spiritual-core audit 2026-06-10. Anchors verified by tracing code paths.
