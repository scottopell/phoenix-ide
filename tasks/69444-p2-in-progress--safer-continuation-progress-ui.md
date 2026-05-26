# Safer continuation progress UI

## Problem

While Phoenix is generating a continuation summary (`awaiting_continuation`), the UI currently looks like a generic working state: the state bar says `summarizing...` and the composer turns into the normal `Stop` affordance. That makes it easy to accidentally interrupt the continuation/compaction mechanism, especially when the user expects the system to be preparing a handoff rather than doing ordinary agent work.

Claude Code's compaction progress bar is a useful reference: the UI should make this phase feel like a distinct, expected handoff operation with visible progress/indeterminate activity. More importantly, continuation generation should not be cancellable at all: cancellation here is a footgun, not a useful user journey.

## Relevant code/specs

- `specs/bedrock/requirements.md`
  - REQ-BED-020: continuation summary generation
  - REQ-BED-021: context exhausted state
  - REQ-BED-023: manual continuation trigger behaves like automatic continuation
- `specs/bedrock/bedrock.allium`
  - `awaiting_continuation` is the in-flight summary state
  - currently documents cancellation rules for other parent states; update if needed so `awaiting_continuation` has no user-cancel transition
- `crates/phoenix-ide/src/state_machine/transition.rs`
  - cancellation transition behavior
- `ui/src/utils.ts`
  - `isAgentWorking` treats `awaiting_continuation` as working
  - `getStateDescription` returns `summarizing...`
- `ui/src/components/StateBar.tsx`
  - renders the state text and context indicator
- `ui/src/components/InputArea.tsx`
  - renders the generic `Stop` button whenever `isAgentWorking(convState)` is true
- `ui/src/pages/ConversationPage.tsx`
  - `handleCancel` currently gates on generic `isAgentWorking(atom.phase)`
  - `handleTriggerContinuation` calls `api.triggerContinuation`

## Proposed direction

1. Make continuation generation non-cancellable by construction.
   - Backend/state machine/API should not support user cancellation while in `awaiting_continuation`.
   - If `POST /api/conversations/:id/cancel` is called in this state, it should not abort the continuation request. Prefer returning a clear non-2xx conflict/invalid-state response or otherwise absorbing without side effects, matching existing API conventions.
   - Update the relevant bedrock Allium/spEARS spec if the current spec implies cancellation is allowed for this state.
2. Expose cancellation capability to the UI as a state-derived concept, not as `isAgentWorking`.
   - Add/adjust a helper such as `canCancelConversationState(state)` so the composer renders `Stop` only for states that the backend actually allows cancellation for.
   - `awaiting_continuation` must return false.
3. Treat `awaiting_continuation` as a first-class UI phase, not just generic agent work.
4. Show an inline compaction/continuation progress indicator while the summary is being generated.
   - An indeterminate progress bar is acceptable unless backend progress data already exists or is added later.
   - Suggested copy: `Compacting conversation...` / `Generating continuation summary...`.
   - Make clear that Phoenix is preparing a new conversation and preserving context.
5. Keep normal agent/tool/sub-agent Stop behavior unchanged.

## Acceptance criteria

- Backend cancellation does not abort continuation generation when the conversation is in `awaiting_continuation`.
- API/state-machine tests cover attempted cancellation in `awaiting_continuation` and assert the continuation request is not interrupted.
- The frontend derives Stop visibility from a cancellation-capability helper rather than generic `isAgentWorking`.
- When `convState.type === 'awaiting_continuation'`, the composer does not show `Stop` and Escape does not call the cancel endpoint.
- When `convState.type === 'awaiting_continuation'`, the UI displays a distinct continuation/compaction in-progress indicator instead of only generic `summarizing...` text.
- Other cancellable working states (`llm_requesting`, `tool_executing`, `awaiting_sub_agents`, etc., as supported by backend semantics) retain their existing Stop behavior.
- Add or update React tests covering:
  - distinct `awaiting_continuation` rendering
  - no cancel endpoint call from click or Escape while in `awaiting_continuation`
  - unchanged normal Stop behavior for cancellable states

## Notes

This task is not just a UI polish change. The pit-of-success API pattern is the important part: because continuation cancellation is a footgun, the backend should not represent it as a supported action, and the frontend should naturally follow from that capability boundary.
