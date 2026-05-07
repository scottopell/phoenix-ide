---
created: 2026-05-07
priority: p2
status: in-progress
artifact: pending
---

# collapse-completed-chains-by-default

## Plan

# Collapse completed chains by default in sidebar

## Summary

Completed chains (where the latest member is in a terminal state) currently render fully expanded in the sidebar, which is noisy for long chains you're done with. This change makes them collapsed by default.

## Context

- **File**: `ui/src/components/ConversationList.tsx`
- `collapsedChains: Set<string>` tracks which chains the user has manually toggled from their default state
- A chain absent from the set → expanded (current hardcoded default)
- "Completed" = latest chain member has display state `'terminal'` (i.e. `presentation_mode === 'done'` or `state.type === 'terminal'`)

## What to change

In `renderChainBlock` (~line 302), replace:

```ts
const collapsed = collapsedChains.has(item.rootId);
```

with:

```ts
const latestMember = item.members.find(m => m.id === item.latestMemberId);
const isCompleted = getConvDisplayState(latestMember) === 'terminal';
// Completed chains default collapsed; toggles flip from that default.
const collapsed = isCompleted ? !collapsedChains.has(item.rootId) : collapsedChains.has(item.rootId);
```

`getConvDisplayState` is already imported from `'../api'`. No other changes needed — `toggleChainCollapsed` already adds/removes from the set, which correctly flips state in either direction regardless of default.

## Acceptance criteria

1. A chain whose latest member is in terminal state renders collapsed in the sidebar on first load
2. Clicking the caret on a completed chain expands it (toggle works)
3. Clicking again re-collapses it
4. Non-completed chains are still expanded by default
5. No regressions on the toggle behavior for non-completed chains


## Progress

