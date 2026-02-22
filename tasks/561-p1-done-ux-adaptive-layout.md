---
id: 561
priority: p1
status: done
title: UX Adaptive Layout Improvements
created: 2025-02-22
requirements:
  - REQ-UI-012
  - REQ-UI-013
  - REQ-UI-014
  - REQ-UI-015
spec: specs/ui/
---

# UX Adaptive Layout Improvements

Implement UX improvements identified in the adaptive layout audit (`docs/ux-audit-adaptive-layout.md`). Requirements are defined in `specs/ui/requirements.md`.

## Scope

This task covers UI improvements that do NOT depend on the Command Palette:

### 1. Conversation State Indicators (REQ-UI-012)

**File:** `ui/src/components/ConversationList.tsx`

- Add visual state indicator (dot) to each conversation in the list
- States: idle (green), working (yellow/pulsing), error (red)
- Backend already exposes `state: ConvState` field on Conversation objects (see `src/db/schema.rs`)
- Map `ConvState` to display state:
  - `Idle` → idle (green)
  - `Error { .. }` → error (red)  
  - All other states → working (yellow/pulsing)

**Polling:** Add periodic refetch of conversation list (every 5-10s) when list page is visible. This reuses existing `/api/conversations` endpoint.

### 2. Per-Conversation Scroll Position Memory (REQ-UI-013)

**File:** `ui/src/pages/ConversationPage.tsx`

- Save scroll position to `localStorage` keyed by conversation ID
- Storage key: `phoenix:scroll:{conversationId}` (extends schema from REQ-UI-011)
- Save on: route change, visibility change (tab switch)
- Restore on: conversation mount (after messages render)
- Edge case: if new messages arrived while away, still restore position but show "jump to newest" affordance

### 3. Desktop Message Width Constraint (REQ-UI-014)

**File:** `ui/src/index.css` (or `MessageList.tsx` styles)

- Add `max-width: 800px` and `margin: 0 auto` to message container
- Pure CSS change, minimal risk
- Ensure code blocks scroll horizontally within constrained width (already have `overflow-x: auto` on pre/code)

### 4. Mobile Bottom Sheet New Conversation (REQ-UI-015)

**Files:** 
- New component: `ui/src/components/NewConversationSheet.tsx`
- Modify: `ui/src/pages/ConversationListPage.tsx` (or wherever "+ New" triggers)

- Replace full-page `/new` navigation with bottom sheet overlay on mobile (< 768px)
- Reuse existing `SettingsFields` component from `NewConversationPage.tsx`
- Bottom sheet behavior:
  - Slides up from bottom
  - Swipe-down to dismiss
  - Backdrop tap to dismiss
  - Current view visible (dimmed) behind sheet
- Desktop (>= 768px) can keep current `/new` page behavior or also use sheet

## Out of Scope

- Conversation search/filter (handled by Command Palette - task 562)
- Sidebar layout on desktop (future enhancement)
- Swipe gesture for list overlay on mobile (future enhancement)

## Acceptance Criteria

- [x] Conversation list shows state indicator dots with correct colors
- [x] State updates within 10 seconds when conversation state changes
- [x] Scroll position preserved when switching between conversations
- [x] "Jump to newest" affordance appears when returning to conversation with new messages
- [x] Messages readable on 1440px+ wide displays (constrained width)
- [x] New conversation on mobile opens as bottom sheet, not full page
- [x] Bottom sheet dismissible via swipe-down or backdrop tap
