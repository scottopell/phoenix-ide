A per-agent sub-agent timeout (`spec.timeout`, distinct from the parent's `AwaitingSubAgents` deadline) is recorded/rendered as a *cancellation* rather than a timeout.

The per-agent timeout timer (runtime.rs, the `timeout_task` spawned per sub-agent) sends `Event::UserCancel { cause: CancelCause::UserRequested }` to the sub-agent when `spec.timeout` fires. The sub-agent routes through its `CancellingTool`/catch-all `UserCancel` arms and notifies the parent with `SubAgentOutcome::Failure { error_kind: Cancelled }` ("Sub-agent timed out"). Because the parent is in `AwaitingSubAgents` (not `CancellingSubAgents`) when a single agent times out, the new `CancelCause::Timeout` outcome-mapping (task 61004) never applies to this source, so a per-agent timeout shows up in history as "cancelled," not "timed out."

Pre-existing (per-agent timeouts always rendered as cancelled); the 61004 cause-mapping work made the inconsistency visible (some timeout sources now map to `TimedOut`, this one doesn't). Surfaced by Codex review of PR #328 (runtime.rs per-agent timer, P2).

Fix sketch: set `cause: CancelCause::Timeout` on the per-agent timer's `UserCancel`, and thread the cause through the SUB-AGENT side so its terminal arms map cause -> outcome (`Timeout` -> `NotifyParent { SubAgentOutcome::TimedOut }`, `UserRequested` -> `Failure{Cancelled}`). That requires the sub-agent's `CancellingTool` to carry the cause (mirroring the parent-side `CancellingSubAgents.cause` threading) so the deferred-until-tool-settles notify can still honor it. Distinct sub-system from 61004's parent-side teardown work.

Severity: fidelity/observability — no wedge, no data loss; a timed-out sub-agent is mislabeled as cancelled in LLM history + UI.
