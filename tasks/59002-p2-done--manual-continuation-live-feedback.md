# Make manual continuation feel safe and visibly start

User feedback from using the context-menu continuation action:

1. The action label `End & summarize now` sounds destructive/scary. It should communicate the actual workflow: summarize the current conversation and continue in a new one. Prefer wording like `Summarize & continue` / `Summarize and continue`.
2. After clicking the action, the current page did not show that continuation generation was underway until refresh. Investigation found the backend is *intended* to emit a live `state_change` SSE for `awaiting_continuation`, and refresh proving the indicator appeared means the state did persist. The specific live-SSE miss remains unexplained. This task hardens the UX so accepted manual continuation commands show progress immediately, while adding targeted diagnostics/coverage to distinguish future “SSE not emitted” vs “SSE emitted but client missed/dropped it” cases.

## Current anchors

- UI label: `ui/src/components/ContextIndicator.tsx`
  - prop comment and tooltip/hint also use “end and summarize” language.
  - button currently renders `End & summarize now`.
- Manual trigger call: `ui/src/pages/ConversationPage.tsx::handleTriggerContinuation` posts `api.triggerContinuation(conversationId)` and relies on SSE for UI state.
- Backend endpoint: `crates/phoenix-ide/src/api/handlers.rs::trigger_continuation` sends `Event::UserTriggerContinuation`.
- State machine intent: `crates/phoenix-state-machine/src/transition.rs` maps `Idle + UserTriggerContinuation` to `AwaitingContinuation` with `PersistState`, `NotifyStateChange`, and `RequestContinuation` effects.
- Existing UI progress indicator: `ui/src/components/InputArea.tsx` renders for `convState.type === 'awaiting_continuation'`.

## Plan

1. Rename the manual continuation action and surrounding copy to non-destructive language.
   - Button text: `Summarize & continue` (or similar).
   - Tooltip/hint should avoid saying the conversation will be “ended”; explain that Phoenix prepares a handoff summary for a continuation conversation.
   - Update `StateBar.test.tsx` expectations for the new accessible name.
2. Make the manual trigger show immediate progress without changing the runtime-command API convention.
   - Keep `POST /trigger-continuation` as an enqueue/acceptance endpoint; this matches other `send_event` handlers and should not be changed to synchronously wait for runtime effects unless a broader API contract change is made.
   - After a successful `api.triggerContinuation(conversationId)`, dispatch a local phase change to `{ type: 'awaiting_continuation', attempt: 1 }`, scoped with `expectedConversationId`, just like message send optimistically dispatches `awaiting_llm`.
   - Let the authoritative SSE/init state reconcile that local phase when it arrives.
3. Add targeted diagnostics and regression coverage for the live-SSE miss.
   - Backend test: manual continuation from idle persists and broadcasts `state_change { state: awaiting_continuation }` before/alongside `RequestContinuation`.
   - Backend instrumentation: when broadcasting the manual continuation `StateChange`, log `conv_id`, `sequence_id`, state name, and `receiver_count` at debug level so a future report can distinguish “not emitted” from “no subscribers / client dropped it”.
   - Frontend test/instrumentation: ensure a live `state_change` with `awaiting_continuation` is parsed and applied to the atom; in dev, existing sequence/epoch replay-drop logs should make a dropped event observable.

## Acceptance criteria

- The manual action no longer contains “End” in visible text or tooltip copy.
- Clicking manual continuation on an idle live conversation shows the existing “Preparing a continuation” progress UI immediately after the command is accepted, without refreshing.
- The progress UI is reconciled by authoritative server state: refresh/reconnect observes the same `awaiting_continuation` state from server state/SSE replay while summary generation is in flight.
- The still-unexplained live-SSE miss is made diagnosable: backend coverage/debug breadcrumbs show whether manual-continuation `awaiting_continuation` state changes were broadcast and whether live subscribers existed.
- Existing context-exhausted continuation behavior still works unchanged after the summary completes.

## Validation

Run focused tests first:

```bash
./dev.py test-ui -- StateBar ContextIndicator InputArea ConversationPage
cargo test trigger_continuation awaiting_continuation
```

Then run the normal gated check:

```bash
./dev.py check
```
