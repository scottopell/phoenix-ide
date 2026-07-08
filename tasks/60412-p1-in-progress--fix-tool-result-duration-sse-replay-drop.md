# Fix tool-result duration SSE replay drop

## Problem

A completed live turn can render blank tool-result cards even though the database contains the tool results. The repro found in `review-browser-auto-pop-heuristics` shows:

- assistant message `2810` contains four `read_file` `tool_use` blocks
- tool-result messages `2811` through `2814` are all persisted
- the live UI showed only the first result, while the later tool cards had no paired result

Root cause hypothesis: after broadcasting each persisted tool-result message, the backend emits a separate `MessageUpdated` carrying `duration_ms`. Those update events allocate fresh sequence IDs after the already-allocated tool-result message sequence IDs. If the first duration update (`2815`) reaches the browser before later tool-result messages (`2812`–`2814`), the client’s global monotonic `lastSequenceId` guard treats those lower-sequence messages as replays and drops them.

## Goal

Make this specific bug impossible without redesigning the SSE architecture.

## Plan

1. **Backend: remove redundant duration update after persisted tool-result broadcast**
   - In `RuntimeExecutor::persist_checkpoint` and the fork-proposal checkpoint path if applicable, stop emitting the follow-up `SseEvent::MessageUpdated { duration_ms }` for tool-result rows whose `display_data` already includes `duration_ms`.
   - Preserve any live/in-flight update behavior that is genuinely needed before persistence; only remove the redundant post-message update that can leapfrog later persisted message rows.

2. **Client regression coverage for replay/drop ordering**
   - Add a reducer-level regression test that simulates:
     - assistant message with multiple tool_use blocks
     - first tool-result message
     - higher-sequence `message_updated` for the first result
     - later lower-sequence tool-result messages
   - The test should capture the desired safe behavior after the patch. If removing the redundant backend update is sufficient for production, still encode the client behavior we want when such an update appears.

3. **Make the UI failure mode visible**
   - Update `ToolUseBlock` to render an explicit fallback when a historical tool_use has no paired result and is not the active running tool, e.g. `result not received` / `missing result`.
   - Keep the active/running elapsed behavior unchanged.
   - Avoid implying tool failure; this is a transcript/rendering integrity warning, not a tool error.

4. **Tests**
   - Backend test or adjusted existing test to prove persisted tool-result rows carry `duration_ms` in `display_data` and no redundant duration `MessageUpdated` is emitted in the post-persist broadcast path.
   - UI component/reducer test for the visible missing-result fallback.
   - Run targeted tests first, then the project’s normal check lane as appropriate.

## Non-goals

- No broad SSE sequence architecture redesign.
- No migration or DB repair; persisted data is already correct for the observed case.
- No changes to stale-tool-result clearing.

## Expected outcome

Live sessions no longer drop later tool-result messages because of duration-update sequence leapfrogging. If a tool_use/result pairing is ever missing for another reason, the UI shows an explicit diagnostic instead of a blank card.
