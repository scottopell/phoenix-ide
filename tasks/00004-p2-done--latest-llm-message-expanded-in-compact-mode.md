# Keep latest LLM summary expanded in compact density

## Problem

When conversation density is set to compact, assistant prose blocks that span multiple lines or exceed the preview limit render as collapsed one-line summaries. That is useful for older history, but the latest finalized LLM message is often the post-work completion summary. Users currently have to click it open every time.

## Goal

In compact density, keep the latest finalized assistant/LLM message’s text rendered in the normal non-collapsed form by default, while preserving compact behavior for older assistant messages and tool strips.

## Proposed implementation

- Add a structural signal from the message list/render-unit layer to `AgentMessage`, e.g. `isLatestAgentMessage` or a more narrowly named `forceExpandedText` prop.
- Compute the latest historical `agent_turn` unit in `MessageList` from the already-built `historicalUnits` array and pass the signal only to that row.
  - Do not treat the live `StreamingMessage` tail as a finalized latest message.
  - Keep existing compact tool-strip behavior unchanged; only assistant text preview collapse should be bypassed.
- In `MessageComponents.tsx`, update the compact text branch so `CollapsibleText` is skipped when the new signal is true.
- Add/adjust tests:
  - `AgentMessage` renders long/multiline prose fully in compact mode when the new prop is set.
  - Existing compact collapse tests remain true by default for older/non-latest assistant messages.
  - Prefer a `MessageList` or render-unit-facing test that verifies only the final historical agent row receives the expanded/default signal, if practical with existing mocks.

## Acceptance criteria

- With density = compact, older long/multiline assistant messages still collapse to one-line previews.
- With density = compact, the latest finalized assistant message renders its text fully by default.
- Tool-use details continue to collapse into the compact pill strip unless the user expands them.
- Streaming behavior is unchanged.
- Relevant UI tests pass.
