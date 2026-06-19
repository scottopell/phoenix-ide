A tool's outcome reaches the executor via a detached background task (`forward_tool_outcome` in runtime/executor.rs), spawned alongside the tool task. If the runtime is shut down or evicted (server restart, conversation eviction) while a forwarder is still pending — the tool task has not yet produced its `ToolExecOutcome` — the forwarder is dropped and the outcome is lost. A conversation that was in `ToolExecuting`/`CancellingTool` then has no outcome to consume.

Surfaced by adversarial QA of task 08692 (cancellation liveness). The cancellation backstop (REQ-BED-005a) does NOT cover this: it bounds a tool that never returns *within a live runtime*, not an outcome lost because the runtime went away. The `late_tool_outcome_in_idle_is_harmless` test covers a late outcome arriving in Idle, but not the forwarder being dropped before it sends.

Investigate:
- Whether runtime shutdown/eviction can drop a pending forwarder before its send (vs. being awaited/drained on shutdown).
- Whether `reset_all_to_idle` on resume fully covers it (transient `ToolExecuting`/`CancellingTool` reset to Idle on startup, so a restarted server is safe), OR whether an eviction-without-restart path can leave a live runtime wedged with no outcome and no waiting-state deadline armed.
- Confirm with a test that drives shutdown while a tool outcome is in flight.

Fix options (pick after investigation):
- A graceful shutdown hook that drains/flushes pending forwarders before the runtime drops.
- Startup reconciliation that re-resolves a conversation found mid-tool-round.
- A timeout fallback so a never-arriving outcome can't wedge a resumed runtime.

Severity: reliability — a possibly-wedged conversation on restart/eviction, bounded by how often shutdown races an in-flight tool. Not a liveness regression of the shipped 08692 fix (that holds within a live runtime); this is the adjacent "runtime went away mid-flight" case.
