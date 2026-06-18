The executor outcome plumbing validates outcomes by current state SHAPE only, not by identity/epoch/liveness. Four concrete bugs in this family; the durable fix is to epoch-tag outcomes, turn sender-drop into a typed failure outcome, and make forced sub-agent teardown follow the real cancellation protocol instead of racing it.

REOPENED: this task was marked done while M1 was still open. H2 and M3 are done; M2 is most likely done (verify); M1 remains and now absorbs the one-writer-reservation bug that was filed separately as 08695.

## H2 — retry timers carry no epoch, never cancelled — DONE
Fixed: `retry_generation` epoch + generation-tagged retry-outcome channel; stale timers are discarded by generation match. (`ScheduleRetry`/`RetryTimeout`.)

## M3 — panicked/aborted tool task wedges the conversation forever — DONE
Fixed: `forward_tool_outcome`/`forward_llm_outcome` map a dropped oneshot sender to a typed `Failed`/`NetworkError` so a sender-drop can't be silently lost; and REQ-BED-005a adds the bounded `CancellingTool` deadline backstop so the "user cancel waits forever" half can't happen. (NOTE: a residual loss mode — the whole forwarder TASK dropped on runtime shutdown/eviction, not the sender — is tracked separately as 08693.)

## M2 — sub-agent UserCancel during ToolExecuting skips AbortTool — VERIFY (likely done)
Original bug: a SubAgentState-specific `UserCancel` arm went straight to `Failed`+`NotifyParent` with no `Effect::AbortTool`, so a Work-mode sub-agent kept mutating the shared worktree after the parent was told it stopped. `UserCancel` is now a shared `CoreEvent` and `ToolExecuting + UserCancel -> CancellingTool + AbortTool` (transition.rs), which sub-agents route through — so this is probably already fixed. Confirm no sub-agent-specific short-circuit to `Failed` remains; if confirmed fixed, close this sub-item.

## M1 — forced sub-agent teardown races the real runtime (OPEN — the remaining work)
`handle_sub_agent_timeout` (and the cancellation backstop `handle_cancelling_sub_agents_timeout`) inject a synthetic `TimedOut` per pending sub-agent and move on WITHOUT waiting for the real sub-agent runtime to actually stop. Two distinct consequences:

- **Result fidelity** (original M1): the real result arrives late (buffered after the parent left the waiting state) and a real *success* already in flight can be overwritten by the synthetic `TimedOut`; can also surface a spurious user-visible `SseEvent::Error` in a later round.
- **One-writer safety** (was task 08695): the synthetic result runs the normal `SubAgentResult` path, which decrements `active_work_subagents` (the REQ-PROJ-008 one-writer counter) BEFORE the real runtime stops. During the straggler window the parent can admit another Work sub-agent onto the same worktree → two writers on one worktree. Narrow window (Work sub-agent + missed deadline/cancel + immediate respawn), but a real invariant break.

The live `TODO(task 61004)` at `executor.rs` (handle_sub_agent_timeout) marks this.

Fix (the "correct shape"): force teardown should follow the real cancellation protocol — enter/stay in `CancellingSubAgents`, send the real cancel, and finalize each agent (record its outcome AND release its one-writer reservation) only on CONFIRMED termination, never on a synthetic guess. The liveness backstop that prevents a never-reporting agent from wedging the drain already exists (REQ-BED-005a `CancellingSubAgentsDeadlineFires`), so the rework can lean on it: the deadline guarantees progress while real results drive finalization. Must preserve the "timed out" vs "cancelled" semantic the LLM history renders.

Found in spiritual-core audit 2026-06-10. Anchors verified by tracing code paths.
