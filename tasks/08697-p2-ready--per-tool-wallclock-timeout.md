Add a safety-net wall-clock timeout around regular tool execution.

Regular tools are awaited in `execute_tool_to_outcome` (crates/phoenix-ide/src/runtime/executor.rs) via `tool_executor.execute(checked, tool_ctx).await` with NO timeout. Only sub-agents have a backstop (`DEFAULT_SUBAGENT_TIMEOUT`, 20 min, REQ-SA-006). A bug in any tool can therefore park a conversation in ToolExecuting indefinitely with no automatic recovery.

This was the amplifier in the commission_review pipe-deadlock incident: a git diff on a large file (lading_payload/src/opentelemetry/trace.rs, ~98KB) deadlocked inside the tool and the conversation sat in ToolExecuting for an hour. The deadlock itself is fixed (drain stdout in a bounded loop), but the missing backstop meant a silent 1-hour hang instead of a logged timeout the state machine recovers from.

Proposal: wrap the tool execute() future in a wall-clock timeout (mirror DEFAULT_SUBAGENT_TIMEOUT). On expiry, fire the tool's CancellationToken, then synthesize a Failed/Aborted ToolExecOutcome so the SM transitions out of ToolExecuting and the conversation recovers. Log at warn with conv_id/tool/id/elapsed.

Open questions to resolve during design:
- Single global timeout vs per-tool override (commission_review and bash can legitimately run minutes; think is instant).
- Interaction with the existing CANCELLATION_DEADLINE / cancelling-tool teardown path.
- Whether the timeout should be cancellation-first (give the tool its 3s grace) then forced.

Defense-in-depth; would not have prevented the deadlock but would have bounded its blast radius from 1 hour to N minutes.
