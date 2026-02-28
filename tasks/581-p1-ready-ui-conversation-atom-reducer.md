---
created: 2026-02-28
number: 581
priority: p1
status: ready
slug: ui-conversation-atom-reducer
title: "ConversationAtom + useReducer at router-level context"
---

# Conversation Atom and Reducer

## Context

Read first:
- `specs/ui/design.md` — "Conversation Atom and Reducer" section
- `specs/ui/design.md` — "Router-Level Context" section
- `specs/ui/design.md` — Appendix A (UI-FM-2, UI-FM-3, UI-FM-5, UI-FM-6)
- `specs/ui/requirements.md` — REQ-UI-020 (Navigation Persistence)

This is the big UI refactor. Currently `ConversationPage` owns 10+ independent `useState`
atoms. All SSE events update them individually. React may or may not batch these updates.
Navigation unmounts everything.

The fix: one `ConversationAtom`, one pure `conversationReducer`, mounted in a React
context at the router level.

## What to Do

### Phase 1: Define types and reducer

Create `ui/src/conversation/atom.ts` (or similar):

1. Define `ConversationAtom` interface per the design spec
2. Define `SSEAction` union type for all actions
3. Implement `conversationReducer` as a pure function
4. Key invariants:
   - `sse_init` replaces breadcrumbs entirely (authoritative)
   - `sse_message` clears `streamingBuffer` atomically
   - `sse_state_change` appends breadcrumbs with `sequenceId` dedup
   - All actions check `lastSequenceId >= event.sequenceId` for idempotency
   - Malformed events become `UIError` values, not thrown exceptions

5. Write unit tests for the reducer — pure function, no React needed:
   - Init replaces all state
   - Message with duplicate sequenceId is no-op
   - State change appends breadcrumb only if sequenceId is new
   - Token events accumulate in buffer
   - Message event clears streaming buffer

### Phase 2: Create router-level context

Create `ui/src/conversation/ConversationProvider.tsx`:

1. `ConversationProvider` wraps the router, holds a `Map<string, ConversationAtom>`
2. `useConversationAtom(convId)` hook returns `[atom, dispatch]`
3. If atom exists for the convId (navigation back), return it — no re-fetch
4. If new, create fresh atom

### Phase 3: Migrate ConversationPage

1. Replace all `useState` atoms with `useConversationAtom(convId)`
2. Replace SSE event handlers with `dispatch(action)` calls
3. Replace prop drilling with context selectors:
   ```typescript
   const { isAgentWorking, currentTool, breadcrumbs, ... } = useConversationSelectors(convId);
   ```
4. Delete: `agentWorking`, `convState`, `stateData`, `breadcrumbs`,
   `contextWindowUsed`, `contextExhaustedSummary` as separate `useState` calls

**The verification step:** if any of those `useState` calls cannot be deleted, the atom
is not canonical and something was missed.

### Phase 4: Update useConnection

`useConnection` becomes a socket lifecycle manager, not a data owner:
- It receives `dispatch` from the atom
- It receives `lastSequenceId` from the atom (for reconnection URL)
- It does NOT own `lastSequenceId` or `seenIdsRef`
- `seenIdsRef` is deleted entirely — replaced by reducer idempotency

### Phase 5: Update child components

Components receive selector outputs instead of raw props:
- `StateBar` receives `phase` from context, not prop-drilled `convState`
- `BreadcrumbBar` receives `breadcrumbs` from context
- `InputArea` receives `isAgentWorking` from context
- `MessageList` receives `messages` from context

## Acceptance Criteria

- `ConversationPage` has zero `useState` calls for conversation data
- `conversationReducer` has unit tests for each action type
- `lastSequenceId` survives navigation (test: navigate away and back, verify no full
  re-fetch in network tab)
- `seenIdsRef` is deleted
- Breadcrumbs are not duplicated on reconnect
- `./dev.py check` passes (if applicable to frontend)
- Manual test: full conversation flow, navigation between conversations, reconnection

## Dependencies

- Task 579 (discriminated union — provides `ConversationState` type the reducer uses)

## Files Likely Involved

- `ui/src/conversation/atom.ts` — NEW: atom type, reducer, selectors
- `ui/src/conversation/ConversationProvider.tsx` — NEW: router-level context
- `ui/src/conversation/index.ts` — NEW: exports
- `ui/src/pages/ConversationPage.tsx` — MAJOR: migrate from useState to context
- `ui/src/hooks/useConnection.ts` — MODIFY: receive dispatch, remove data ownership
- `ui/src/components/StateBar.tsx` — MODIFY: consume from context
- `ui/src/components/BreadcrumbBar.tsx` — MODIFY: consume from context
- `ui/src/components/InputArea.tsx` — MODIFY: consume from context
- `ui/src/App.tsx` — MODIFY: wrap router with ConversationProvider
