---
created: 2026-02-28
number: 579
priority: p1
status: ready
slug: ui-discriminated-union-state
title: "ConversationState discriminated union with satisfies never"
---

# UI Discriminated Union State

## Context

Read first:
- `specs/ui/design.md` — "Typed Conversation State" section
- `specs/ui/design.md` — Appendix A (UI-FM-1, UI-FM-7)
- `specs/ui/requirements.md` — REQ-UI-007

UI-FM-1: `ConversationState.type` is `string`. Components match with `===` and
`startsWith()`. A catch-all maps unrecognized states to a generic indicator. New backend
variants are silently absorbed.

UI-FM-7: All fields are optional on all variants. `current_tool` is accessible in
`idle` state. The compiler can't distinguish intentional access from defensive `?.`.

## What to Do

1. **Replace `ConversationState` in `ui/src/api.ts`** (or `types.ts`) with a
   discriminated union:

   ```typescript
   export type ConversationState =
     | { type: 'idle' }
     | { type: 'awaiting_llm' }
     | { type: 'llm_requesting'; attempt: number }
     | { type: 'tool_executing'; current_tool: ToolCall; remaining_tools: ToolCall[] }
     | { type: 'awaiting_sub_agents'; pending: PendingSubAgent[]; completed_results: SubAgentResult[] }
     | { type: 'awaiting_continuation'; attempt: number }
     | { type: 'cancelling' }
     | { type: 'cancelling_tool'; current_tool: ToolCall }
     | { type: 'cancelling_sub_agents'; pending: PendingSubAgent[] }
     | { type: 'context_exhausted'; summary: string }
     | { type: 'error'; message: string }
     | { type: 'terminal' };
   ```

2. **Add `isAgentWorking` selector** as a pure function with exhaustive switch:

   ```typescript
   function isAgentWorking(state: ConversationState): boolean {
     switch (state.type) {
       case 'idle': case 'error': case 'terminal': case 'context_exhausted':
         return false;
       case 'awaiting_llm': case 'llm_requesting': case 'tool_executing':
       case 'awaiting_sub_agents': case 'awaiting_continuation':
       case 'cancelling': case 'cancelling_tool': case 'cancelling_sub_agents':
         return true;
       default: state satisfies never; return false;
     }
   }
   ```

3. **Fix every consumer** that accesses `ConversationState`:
   - `StateBar.tsx` — replace string matching with exhaustive switch + `satisfies never`
   - `BreadcrumbBar.tsx` — replace `convState === 'tool_executing'` with switch
   - `ConversationPage.tsx` — replace `agentWorking` useState with `isAgentWorking()`
   - `InputArea.tsx` — derive cancel/send from state, not from separate boolean
   - Any other component accessing `convState` or `stateData`

4. **Delete `agentWorking` useState** from `ConversationPage`. If the delete causes
   compile errors, trace each one and replace with `isAgentWorking(convState)`. If it
   cannot be deleted, the refactor is incomplete.

5. **Handle the SSE → type conversion**: the backend sends a flat JSON object. Write a
   `parseConversationState(raw: unknown): ConversationState` function that validates and
   narrows to the correct variant. Unknown `type` values should produce a typed error,
   not be silently absorbed.

## Acceptance Criteria

- `ConversationState` is a discriminated union (not a flat interface)
- Every switch on `state.type` has `satisfies never` at the default
- `agentWorking` useState is deleted from ConversationPage
- No `?.current_tool` access outside of `tool_executing`/`cancelling_tool` variants
- No string comparisons against state type (no `=== 'tool_executing'` outside switches)
- TypeScript compiles with strict mode
- App works correctly (manual test: send a message, watch state transitions)

## Dependencies

- None (independent of backend tasks, can start immediately)

## Files Likely Involved

- `ui/src/api.ts` or `ui/src/types.ts` — ConversationState type definition
- `ui/src/components/StateBar.tsx` — state display
- `ui/src/components/BreadcrumbBar.tsx` — state matching
- `ui/src/pages/ConversationPage.tsx` — agentWorking, state consumption
- `ui/src/components/InputArea.tsx` — cancel/send logic
