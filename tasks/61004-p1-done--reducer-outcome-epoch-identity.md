The executor outcome plumbing validated outcomes by current state SHAPE only, not by identity/epoch/liveness. Four concrete bugs in this family; the durable fix is to epoch-tag outcomes, turn sender-drop into a typed failure outcome, and make forced sub-agent teardown follow the real cancellation protocol instead of racing it.

All four are now resolved (the last, M1, in the same change that reworked sub-agent forced-teardown). Remaining adjacent gaps are tracked as their own follow-ups (08695, 08693).

## H2 — retry timers carry no epoch, never cancelled — DONE
`retry_generation` epoch + generation-tagged retry-outcome channel; stale timers are discarded by generation match (`ScheduleRetry`/`RetryTimeout`).

## M3 — panicked/aborted tool task wedges the conversation forever — DONE
`forward_tool_outcome`/`forward_llm_outcome` map a dropped oneshot sender to a typed `Failed`/`NetworkError` so a sender-drop can't be silently lost; REQ-BED-005a adds the bounded `CancellingTool` deadline backstop so the "user cancel waits forever" half can't happen. (Residual: a forwarder TASK dropped on runtime shutdown/eviction — not the sender — is tracked as 08693.)

## M2 — sub-agent UserCancel during ToolExecuting skips AbortTool — DONE
`UserCancel` is a shared `CoreEvent`; the sub-agent `ToolExecuting + UserCancel -> CancellingTool + AbortTool` arm routes through `CancellingTool` and defers `NotifyParent` until the tool settles, so a Work-mode sub-agent no longer keeps mutating the shared worktree after the parent was told it stopped. No sub-agent-specific short-circuit to `Failed` remains.

## M1 — forced sub-agent teardown races the real runtime — DONE
Forced teardown (the `AwaitingSubAgents` completion timeout and the cancellation backstop) used to fabricate a synthetic `TimedOut` per pending agent and move on without waiting for the real runtime to stop — overwriting a real success and releasing the REQ-PROJ-008 one-writer reservation before the runtime stopped (the latter was briefly filed as a separate 08695, folded back in here).

Now: forced teardown follows the real cancellation protocol. A typed `CancelCause { UserRequested, Timeout }` rides on `UserCancel` and is stamped onto `CancellingSubAgents`; the completion timeout injects `UserCancel { cause: Timeout }` (the transition emits the real `CancelSubAgents`) rather than fabricating results. Real results drain through `CancellingSubAgents` and finalize each agent — recording its outcome AND releasing its one-writer reservation — only on confirmed termination; the REQ-BED-005a `CancellingSubAgentsDeadlineFires` backstop presumes a never-reporting agent dead (release is then safe). Outcome + terminal are cause-aware: `Timeout` records `TimedOut` (the deadline is a hard contract — even a late success) and resumes the parent (`-> LlmRequesting`, report-and-continue); `UserRequested` keeps the reported outcome and stops (`-> Idle`). `spawn_tool_id` is threaded through `CancellingSubAgents` so results update the real `spawn_agents` tool message instead of minting an orphaned `tool_result`.

## Follow-ups (separate tasks)
- 08695 — a per-agent `spec.timeout` (distinct from the parent's `AwaitingSubAgents` deadline) is still tagged `UserRequested` at the per-agent timer, so it renders as "cancelled" rather than "timed out"; needs cause threaded through the sub-agent's own `CancellingTool`.
- 08693 — forwarder task lost on runtime shutdown/eviction (M3 residual).
- `persist_sub_agent_results`' `None` branch mints a `tool_result` with a random id — a latent orphan for the pre-existing "spawn_agents wasn't the last tool" case; could be hardened to never emit a tool_result without a tool_use.

Found in spiritual-core audit 2026-06-10. Anchors verified by tracing code paths.
