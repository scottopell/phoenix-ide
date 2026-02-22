---
id: 564
priority: p1
status: in-progress
title: Command Palette Implementation
created: 2025-02-22
requirements:
  - REQ-CP-001
  - REQ-CP-002
  - REQ-CP-003
  - REQ-CP-004
  - REQ-CP-005
  - REQ-CP-006
  - REQ-CP-007
  - REQ-CP-008
spec: specs/command-palette/
---

# Command Palette Implementation

Implement the Command Palette feature. This is a keyboard-driven interface for quick navigation and actions.

## Source of Truth

**Read these specs thoroughly before implementing:**

- `specs/command-palette/requirements.md` — EARS-format requirements (REQ-CP-001 through REQ-CP-008)
- `specs/command-palette/design.md` — Architecture, state machine, component interfaces
- `specs/command-palette/executive.md` — Status tracking

## Overview

The Command Palette provides:
- **Single shortcut** (`Ctrl/Cmd+P`) to open from anywhere
- **Two modes**: Search mode (default) and Action mode (`>` prefix)
- **Fuzzy matching** for filtering results
- **Full keyboard navigation** (arrows, Enter, Escape)
- **Desktop-only** for initial release (REQ-CP-008)

## Implementation Checklist

### Phase 1: Core Component Structure

- [x] Create `ui/src/components/CommandPalette/CommandPalette.tsx`
- [x] Create `ui/src/components/CommandPalette/CommandPaletteInput.tsx`
- [x] Create `ui/src/components/CommandPalette/CommandPaletteResults.tsx`
- [x] Create `ui/src/components/CommandPalette/index.ts` (exports)
- [x] Create `ui/src/components/CommandPalette/types.ts` (interfaces)
- [x] Add to `ui/src/App.tsx` at root level (inside router, rendered always)

### Phase 2: State Machine (REQ-CP-001, REQ-CP-002)

- [x] Implement state types per `design.md` "State Machine" section:
  ```typescript
  type ClosedState = { status: 'closed' };
  type OpenState = {
    status: 'open';
    mode: 'search' | 'action';
    query: string;
    selectedIndex: number;
    results: PaletteItem[];
  };
  type PaletteState = ClosedState | OpenState;
  ```
- [x] Implement `transition(state, event)` pure function
- [x] Events: `OPEN`, `CLOSE`, `SET_QUERY`, `SELECT_NEXT`, `SELECT_PREV`, `CONFIRM`
- [x] Mode derived from query: starts with `>` → action mode, else search mode
- [x] `SET_QUERY` recomputes results and resets `selectedIndex` to 0

### Phase 3: Global Keyboard Shortcut (REQ-CP-001)

- [x] Add `useEffect` in `CommandPalette` to listen for `Cmd+P` / `Ctrl+P`
- [x] Prevent default browser behavior (Cmd+P = print)
- [x] Only register on desktop (check viewport or `window.matchMedia`)
- [x] Dispatch `OPEN` event to state machine

### Phase 4: Source Interface (REQ-CP-006)

- [x] Define `PaletteSource` interface:
  ```typescript
  interface PaletteSource {
    id: string;
    category: string;
    search(query: string): PaletteItem[];
    onSelect(item: PaletteItem): void;
  }
  
  interface PaletteItem {
    id: string;
    title: string;
    subtitle?: string;
    icon?: React.ReactNode;
    metadata?: unknown;
  }
  ```
- [x] Implement `ConversationSource`:
  - Empty query → recent conversations (sorted by `updated_at`)
  - With query → fuzzy match on `slug`
  - `onSelect` → `navigate(`/c/${slug}`)`
  - Show state indicator icon (green/yellow/red dot)

### Phase 5: Action Interface (REQ-CP-007)

- [x] Define `PaletteAction` interface:
  ```typescript
  interface PaletteAction {
    id: string;
    title: string;
    category?: string;
    shortcut?: string;
    icon?: React.ReactNode;
    handler: () => void;
  }
  ```
- [x] Implement built-in actions:
  - `new-conversation` — opens new conversation (navigate to `/` or trigger sidebar form)
  - `go-to-list` — navigate to `/`
  - `archive-current` — archive current conversation (if on `/c/:slug`)
- [x] Filter actions by fuzzy match on `title`

### Phase 6: Search Mode Behavior (REQ-CP-003)

- [x] When `mode === 'search'`:
  - Query all registered sources
  - Merge and group results by `category`
  - Rank by match quality (prefer prefix matches)
- [x] Empty query shows defaults from each source
- [x] Selecting result calls `source.onSelect(item)` and closes palette

### Phase 7: Action Mode Behavior (REQ-CP-004)

- [x] When `mode === 'action'` (query starts with `>`):
  - Strip `>` prefix for filtering
  - Show only actions, not search sources
  - Display shortcut hints where available
- [x] Selecting action calls `action.handler()` and closes palette
- [x] Empty action query shows all actions grouped by category

### Phase 8: Keyboard Navigation (REQ-CP-005)

- [x] `ArrowDown` / `Ctrl+N` → `SELECT_NEXT`
- [x] `ArrowUp` / `Ctrl+P` → `SELECT_PREV`
- [x] `Enter` → `CONFIRM` (select current item)
- [x] `Escape` → `CLOSE`
- [x] Keep input focused while navigating
- [x] Visually highlight selected result (`.selected` class)
- [x] Wrap selection at boundaries (optional, or clamp)

### Phase 9: UI Polish

- [x] Centered modal on desktop (~600px wide)
- [x] Backdrop click closes palette
- [x] Smooth open/close animation (CSS transform/opacity)
- [x] Input placeholder: "Search conversations..." / "Type > for actions"
- [x] Result item styling: icon, title, subtitle, shortcut hint
- [x] Category headers between grouped results
- [x] Max visible results: 8-10 without scroll

### Phase 10: Fuzzy Matching

- [x] Implement simple fuzzy match (substring or prefix)
- [x] Score matches: exact > prefix > substring > fuzzy
- [x] Sort results by score descending, then by recency

## Files to Create

```
ui/src/components/CommandPalette/
├── index.ts
├── types.ts              # PaletteSource, PaletteAction, PaletteItem, PaletteState
├── stateMachine.ts       # transition function, state types
├── CommandPalette.tsx    # Main component, keyboard listener, renders overlay
├── CommandPaletteInput.tsx
├── CommandPaletteResults.tsx
├── sources/
│   └── ConversationSource.ts
├── actions/
│   └── builtInActions.ts
└── CommandPalette.css
```

## Files to Modify

- `ui/src/App.tsx` — render `<CommandPalette />` at root

## Testing the Implementation

After implementing, verify each requirement:

### REQ-CP-001: Single Global Shortcut
```
1. Load app on desktop (> 1024px viewport)
2. Press Cmd+P (Mac) or Ctrl+P (Windows/Linux)
3. ✓ Palette opens with input focused
4. Press Escape
5. ✓ Palette closes
6. Click outside palette
7. ✓ Palette closes
```

### REQ-CP-002: Prefix-Based Mode Switching
```
1. Open palette
2. Type "test"
3. ✓ Shows conversation results (search mode)
4. Clear and type ">test"
5. ✓ Shows action results (action mode)
6. Delete the ">"
7. ✓ Switches back to search mode
```

### REQ-CP-003: Search Mode Behavior
```
1. Open palette (empty query)
2. ✓ Shows recent conversations
3. Type partial slug name
4. ✓ Results filter to matching conversations
5. ✓ Each result shows state indicator (dot)
6. Select a conversation (Enter)
7. ✓ Navigates to that conversation
8. ✓ Palette closes
```

### REQ-CP-004: Action Mode Behavior
```
1. Open palette, type ">"
2. ✓ Shows all available actions
3. Type ">new"
4. ✓ Shows "New Conversation" action
5. ✓ Shortcut hint visible if defined
6. Select action (Enter)
7. ✓ Action executes
8. ✓ Palette closes
```

### REQ-CP-005: Keyboard Navigation
```
1. Open palette with results showing
2. Press ArrowDown
3. ✓ Selection moves to next item
4. Press ArrowUp
5. ✓ Selection moves to previous item
6. Press Enter
7. ✓ Selected item activates
8. ✓ Input stays focused throughout
```

### REQ-CP-006: Extensible Source Interface
```
1. Review ConversationSource implementation
2. ✓ Implements PaletteSource interface
3. ✓ search() returns PaletteItem[]
4. ✓ onSelect() handles selection
```

### REQ-CP-007: Extensible Action Interface
```
1. Review builtInActions implementation
2. ✓ Each action has id, title, handler
3. ✓ Actions are filterable
4. ✓ handler() executes correct behavior
```

### REQ-CP-008: Desktop-Only Initial Scope
```
1. Resize viewport to mobile (< 768px)
2. Press Cmd+P
3. ✓ Nothing happens (shortcut not registered)
4. ✓ No palette UI rendered
5. Resize to desktop (> 1024px)
6. Press Cmd+P
7. ✓ Palette opens
```

## Completion Checklist

Before marking this task done, verify ALL of the following:

- [x] All implementation checklist items complete
- [x] All 8 requirements tested per "Testing the Implementation" section
- [x] No TypeScript errors (`npm run typecheck` or build)
- [x] No console errors during normal operation
- [x] Palette doesn't break existing functionality
- [x] Code follows project conventions (see `AGENTS.md`)
- [x] Update `specs/command-palette/executive.md` status for each REQ-CP-*

## Out of Scope

- Mobile support (future enhancement)
- File search within projects
- Settings/preferences access
- Command history
- Tab to autocomplete
