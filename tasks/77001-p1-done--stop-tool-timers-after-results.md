# Stop inline tool timers when results render

## Problem

On the conversation page, inline tool cards start showing a live elapsed counter when a tool call begins, but after the tool output is visible the counter can keep incrementing indefinitely for most tool types. Reloading the page fixes the display, so the persisted data is likely correct and the bug is in the live/SSE rendering path.

Observed example: completed `read_file` and `search` cards display their output while the header still shows `• 17s`, `• 12s`, etc., and those values keep ticking upward until reload.

## Likely area

`ui/src/components/MessageComponents.tsx`:

- `ToolUseBlockImpl` derives `durationMs` from `result?.display_data.duration_ms`.
- The live counter is shown while `result == null && toolStartedAtMs != null`.
- Completed cards with visible output but ticking headers suggest the live incremental path can render a tool result without causing the paired `result` prop / lifecycle condition to transition the header from live elapsed to static completed duration.
- The issue is transient after reload, so compare the live SSE/pending-event merge path with the initial persisted-message path.

Related helpers/tests:

- `buildToolResults(messages)` in `MessageComponents.tsx`
- `AgentMessage` tool-result pairing in `MessageComponents.tsx`
- `ui/src/components/MessageComponents.test.tsx`

## Plan

1. Reproduce with a component/SSE test: render an agent message containing a tool use and a `display_data.tool_starts` entry, then deliver the matching tool result through the live update path. Assert that once output is visible:
   - `.tool-block-elapsed` is absent
   - the completed status/duration is shown when `display_data.duration_ms` exists
   - advancing fake timers does not change the completed card header
2. Trace why live-rendered completed cards still satisfy the in-flight timer condition. Fix the state/pairing model so a visible tool result structurally means the tool is no longer considered in flight.
3. Preserve reconnect/reload behavior: genuinely in-flight tools with `toolStartedAtMs` and no result should keep ticking, and persisted completed tools should remain static.
4. Run the focused UI tests and any relevant type checks via `./dev.py` once in Work mode.

## Acceptance criteria

- Completed inline tool cards never show a live elapsed counter.
- Live elapsed counters still tick for tool calls without results.
- The regression is covered by an automated test that fails against the current live-update behavior.
