# Add bash inspector affordance to bash tool output

## Goal

Make the process inspector reachable directly from bash tool result rendering, not only from the Work Scope pane.

Today, `ui/src/components/WorkScopePanel.tsx` renders an `inspect →` affordance for bash handles via `ViewerSlotContext.openInspect(scopeKey, handleId)`. The conversation transcript’s bash tool output renderer (`BashResponseView` in `ui/src/components/MessageComponents.tsx`) already displays the handle id but does not provide the same inspector entry point.

## Plan

1. Thread the conversation work-scope key into message rendering:
   - Add an optional `workScopeKey` prop through `ConversationPage` → `ConversationNavStack`/`MessageList` → `AgentMessage` → `ToolUseBlock` → `BashResponseView`.
   - Use `conversation.work_scope_key` from `ConversationPage`, not the conversation slug. The inspector URL requires the backend work-scope key paired with the bash handle id.

2. Add an inspector affordance to bash responses:
   - In `BashResponseView`, when the parsed bash response has a string `handle` and a `workScopeKey` is available, render an `inspect →` button alongside the existing status/handle metadata.
   - On click, call `useViewerSlot().openInspect(workScopeKey, handle)`.
   - Preserve current behavior for legacy/plain bash outputs, bash errors without a handle, and surfaces without a viewer slot / work-scope key.

3. Keep the hook safe:
   - Follow the existing WorkScopePanel pattern: isolate the hook call in a small child component that is only rendered when inspection is possible, so non-inspectable contexts (tests/share pages/etc.) do not accidentally require a `ViewerSlotProvider`.

4. Tests:
   - Add/adjust `MessageComponents.test.tsx` coverage for a structured bash tool result with a handle and `workScopeKey`, asserting the inspect affordance renders and updates the URL to `viewer=inspect&scope=<workScopeKey>&handle=<handle>` when clicked.
   - Add/verify coverage that the affordance is absent when `workScopeKey` is missing or the bash response has no handle.

## Acceptance criteria

- Bash tool outputs in the conversation transcript expose the same process inspector entry point as Work Scope bash rows.
- Clicking the affordance opens the existing `ProcessInspectorPanel` viewer slot for the correct `(scope_key, handle_id)` pair.
- Existing non-conversation/share/test render paths remain safe when no work-scope key or viewer provider is present.
- Relevant UI tests pass.
