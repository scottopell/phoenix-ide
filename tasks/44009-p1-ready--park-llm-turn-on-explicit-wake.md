# Park an LLM tool round on an explicit wake

## Follow-up scope

Implement the explicit wake-registration behavior specified by REQ-WAKE-001 and ADR-011: when the model explicitly commits to waiting on a durable handle, checkpoint the provider-valid tool round and do not invoke the LLM again until terminal delivery is accepted.

This is deliberately separate from the P0 incorrect-auto-wake bug. Fixing automatic wake creation and stale terminal replay does not require introducing park-on-wake behavior.

## Complexity assessment

**Medium-high, bounded to the tool/result/state-machine seam.** The durable wake scheduler and delivery substrate already exist. The missing capability is a typed distinction between an ordinary tool result that should continue the LLM loop and an explicit durable wait receipt that should checkpoint and park.

Do not parse `wake_registration` from display JSON to control the state machine. Thread a typed turn disposition through `ToolOutput`, `ToolResult`/`ToolExecOutcome`, and `handle_core_tool_complete` so invalid combinations cannot be represented.

## Proposed behavior

- Provide the explicit unified durable-wait surface required by `specs/wake-contracts/requirements.md`, or an equivalently explicit typed first slice if repository constraints require it.
- Successful registration is the model's explicit commitment that no more work is needed until terminal delivery.
- Execute already-issued sibling tool calls and persist a provider-valid complete tool round before entering Idle.
- Do not emit `Effect::RequestLlm` after the successful registration round.
- Registration failure returns as an ordinary tool error and continues the LLM loop; it cannot strand the conversation.
- Multiple successful registrations in one round coalesce into at most one resumed LLM request.
- Pending wakes do not redefine `ConvState::is_busy()` and do not require a first-class `AwaitingWake` state.

## Acceptance evidence

- Explicit durable registration on a live bash or tmux handle checkpoints the round and produces no intervening LLM request.
- Terminal completion later resumes exactly once with committed durable observations.
- User steering, cancellation, continuation, and restart recovery preserve provider-valid history and wake ownership.
- Multi-tool and registration-failure regression tests prove deterministic behavior.
- Normative specs and executive/current-reality documentation are aligned, and `./dev.py check` passes.

## Non-goals

- Do not fix the implicit auto-wake/stale foreground-consumption bug here; that is P0 task 44008.
- Do not pause, serialize, or resume an in-flight provider stream.
- Do not add `AwaitingWake` or replace the durable workflow engine.
