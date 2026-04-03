---
created: 2026-02-28
priority: p2
status: done
artifact: completed
---

# Breadcrumb Result Summaries

## Problem

Breadcrumb tooltips currently show tool *input* (what the tool was asked to do).
After the tool completes, the tooltip should show the tool *result* (what it produced).
This answers transparency question 9: "For each tool call, what was the output?"

Related: Task 529 (original issue).

## Context

Read first:
- `specs/ui/requirements.md` — REQ-UI-007 (Agent Activity Indicators), Transparency
  Contract (questions 4 and 9)
- `specs/ui/design.md` — "Typed Conversation State" section (breadcrumb types)

The breadcrumb bar was refactored in tasks 579/581 to use a single-writer reducer with
`sequenceId` dedup. The `Breadcrumb` type in `ui/src/types.ts` currently has:

```typescript
interface Breadcrumb {
  type: 'user' | 'llm' | 'tool' | 'subagents';
  label: string;
  toolId?: string;
  sequenceId?: number;
  preview?: string;  // tooltip content — currently always tool INPUT
}
```

## What to Do

1. **Add `resultSummary` field to `Breadcrumb`** — absent during tool execution,
   populated when the tool result message arrives:

   ```typescript
   interface Breadcrumb {
     // ... existing fields ...
     resultSummary?: string;  // populated on tool completion
   }
   ```

2. **In the conversation reducer**, when an `sse_message` action arrives for a tool
   result message, find the matching breadcrumb by `toolId` and update its
   `resultSummary` with a truncated summary of the result.

3. **In `BreadcrumbBar.tsx`**, update the tooltip rendering:
   - If `resultSummary` exists: show it (completed tool — "bash: success, compiled in 4.2s")
   - If only `preview` exists: show it (in-progress tool — "bash: running `cargo build`...")

4. **Derive the summary** from the tool result content. Keep it short (one line, ~80
   chars max). For bash: include exit code and first line of output. For patch: include
   file path and operation. For other tools: first line of output truncated.

## Acceptance Criteria

- Hovering a completed tool breadcrumb shows result, not input
- Hovering an in-progress tool breadcrumb still shows input
- Summary is concise (one line, truncated if needed)
- No regression in breadcrumb click-to-scroll behavior
- `./dev.py check` passes

## Files Likely Involved

- `ui/src/types.ts` — Breadcrumb interface
- `ui/src/conversation/atom.ts` — reducer, breadcrumb update on tool result
- `ui/src/components/BreadcrumbBar.tsx` — tooltip rendering
