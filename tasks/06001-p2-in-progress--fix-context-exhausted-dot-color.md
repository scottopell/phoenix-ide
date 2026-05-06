---
created: 2026-05-06
priority: p2
status: in-progress
artifact: pending
---

# fix-context-exhausted-dot-color

## Plan

# Fix: context_exhausted indicator dot shows grey instead of purple

## Problem
In the conversation list sidebar, conversations in the `context_exhausted` state show a **grey** indicator dot — the same color as `terminal` (completed). Grey signals "nothing happening here", but `context_exhausted` requires a user decision (continue? pivot?), making it semantically identical to `awaiting_task_approval` / `awaiting_user_response`, which both correctly show **purple**.

## Root Cause
`ui/src/api.ts`, `getDisplayState()`, line 157:
```typescript
case 'context_exhausted': return 'terminal';  // wrong — renders grey
```

## Changes

### 1. `ui/src/api.ts` — fix the display state mapping
```typescript
// Before:
case 'context_exhausted': return 'terminal';

// After:
case 'context_exhausted': return 'awaiting_approval';
```

### 2. `ui/src/components/ConversationList.tsx` — fix the tooltip
The tooltip `switch` on display state has no `context_exhausted` arm so it falls through to `default: return s`, showing the raw key. Add a friendly label:
```typescript
case 'awaiting_approval':
  return 'Awaiting approval';
// becomes — no change needed to this arm since context_exhausted now maps here
// but add to the title switch in getDisplayState tooltip block:
case 'context_exhausted':
  return 'Context full';
```
Actually — since the tooltip switch is over the *display state* (not the raw state type), and `context_exhausted` now maps to `awaiting_approval`, the existing `case 'awaiting_approval': return 'Awaiting approval'` arm will fire. That's acceptable but not ideal — the tooltip should distinguish "awaiting approval" from "context full". 

So the tooltip switch in ConversationList.tsx should instead be written over the raw `conv.state?.type` for more precision, or we can add a separate display for context_exhausted. Simplest approach: keep the dot CSS class change (fixes the grey), and update the tooltip to say "Context full" for `context_exhausted` state type specifically.

## Acceptance Criteria
- Conversations with `context_exhausted` state show a **purple** dot in the sidebar (matching `awaiting_task_approval` and `awaiting_user_response`)
- Hovering that dot shows a sensible tooltip (e.g. "Context full")
- No other states are affected
- UI-only change; no Rust restart needed (Vite hot reloads)


## Progress

