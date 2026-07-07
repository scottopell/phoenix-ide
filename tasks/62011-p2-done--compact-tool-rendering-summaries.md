# Improve compact-mode tool rendering with useful summaries

## Problem

In compact conversation density, tool calls currently collapse into a pill strip that mostly shows repeated tool names (`search → read_file → read_file`, or individual cards showing only `search`). On mobile this is especially unhelpful: the compact view hides the information users need to understand what the agent is doing without expanding every tool.

Expanded tool rendering is acceptable; the gap is specifically the compact collapsed representation.

Current code paths:

- `ui/src/components/agentTurnToolStrip.ts` derives compact strip items with only `{ name, toolId, isSubAgent, hasResult, isError }`.
- `ui/src/components/MessageComponents.tsx` renders `CompactToolStrip` labels as `item.name` only.
- `ToolUseBlock` already has formatting helpers and structured output renderers that can inform concise summaries.

## Goal

Make compact tool rendering show useful, scannable context for the common tools that dominate normal conversations, while preserving the ability to expand to full tool detail.

## Scope

This task should cover the top compact-mode pain points, not every tool in the registry.

### In scope: tool-specific summaries

Implement explicit compact summaries for these common tools:

1. `search`
2. `read_file`
3. `bash`
4. `patch`
5. `keyword_search`
6. `browser_navigate`
7. `browser_click`
8. `browser_type`
9. `browser_wait_for_selector`
10. `browser_eval`
11. `browser_take_screenshot`
12. `browser_recent_console_logs`
13. `spawn_agents`
14. `skill`
15. `propose_task`

These are enough to address the screenshot class of failure and the main browser/shell/code-edit workflows without turning this into a full audit of every current and future tool.

### In scope: generic fallback

For any other tool, show a generic but useful fallback:

- tool name
- first meaningful scalar input value, or a compact `key: value` pair
- status (`pending`, success, error)

This prevents unknown or less-common tools from degrading to name-only rendering.

### Out of scope

- Bespoke summaries for every tool in the registry.
- Rendering full outputs in compact mode.
- Changing expanded tool rendering behavior except for shared helper extraction.
- Adding new backend display data fields unless a frontend-only summary is impossible.

## Proposed implementation

1. Extend compact strip derivation to include a short input/result summary per tool.
   - Examples:
     - `search: compact|Tool|tool in ui/src`
     - `read_file: MessageComponents.tsx:711`
     - `bash: ./dev.py check`
     - `browser_click: .submit-button`
     - `patch: 1 file / 3 changes` where available from input shape
   - Reuse or extract existing input formatting (`formatToolInput` / `summarizeToolInput`) rather than duplicating logic where practical.

2. Add result-aware summaries for the scoped tools when the result is available and cheap to derive.
   - Examples:
     - `search: 12 matches in 4 files`
     - `keyword_search: 5 relevant files`
     - `read_file: 200 lines`
     - `bash: exited 0` / `running` / `failed`
     - `browser_recent_console_logs: 3 errors, 2 warnings`
   - Keep this best-effort; fallback to the input summary instead of expanding heavy output parsing everywhere.

3. Update compact UI from name-only pills to richer compact rows/cards.
   - Preserve ordering and click-to-expand behavior.
   - Make repeated tools distinguishable in a narrow mobile viewport.
   - Show status (`pending`, success, error) inline with the summary.
   - Avoid rendering full outputs in compact mode.

4. Add tests for compact derivation and rendering.
   - `deriveToolStripItems` includes summary fields for common tool inputs.
   - Repeated `search` / `read_file` calls produce distinguishable labels.
   - Result-derived summaries appear for search/read_file/bash where fixtures are straightforward.
   - Click-to-expand still reveals full `ToolUseBlock` and scrolls to the selected tool.

## Acceptance criteria

- Compact mode no longer displays only a list of repeated tool names for common tools.
- The screenshot scenario (`search → read_file → read_file` plus multiple search cards) becomes understandable without expanding: each entry includes the query/path/result count or similar context.
- Mobile layout remains readable at ~390px width.
- Full mode and expanded compact tool detail are unchanged except for any shared helper refactor.
- Existing tests pass, with new coverage for compact tool summaries.

## Validation

- Run targeted UI tests for `agentTurnToolStrip` and `MessageComponents`.
- Manually verify compact density in a mobile-sized viewport with repeated `search` and `read_file` calls.
- Run `./dev.py check` before commit.
