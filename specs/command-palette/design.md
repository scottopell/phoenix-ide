# Command Palette - Design Document

## Architecture Overview

The command palette is a React component with its own state machine, integrated at the app root level. It intercepts the global keyboard shortcut and renders as a portal overlay.

```
┌─────────────────────────────────────────────┐
│                 App Root                        │
│  ┌─────────────────────────────────────────┐  │
│  │           CommandPalette                    │  │
│  │  ┌───────────────┐  ┌──────────────────┐  │  │
│  │  │  InputField   │  │  ResultsList       │  │  │
│  │  └───────────────┘  └──────────────────┘  │  │
│  └─────────────────────────────────────────┘  │
│                                               │
│  ┌─────────────────────────────────────────┐  │
│  │              Router Outlet                  │  │
│  └─────────────────────────────────────────┘  │
└─────────────────────────────────────────────┘
```

### Data Availability Analysis

**Conversation data:** Already available. The app fetches conversations via `/api/conversations` and stores them in the `appMachine` XState context (`context.conversations`). The palette can read directly from this existing state—no new backend queries needed.

**Conversation state:** Already exposed. Each `Conversation` object includes a `state` field with the full `ConvState` enum. The UI currently ignores this; the palette (and REQ-UI-012 state indicators) will consume it.

**Actions:** Client-side only. Actions like "New Conversation", "Archive Current" are handlers that already exist in page components. We'll extract these into a registry that the palette can query.

**Where state lives today:**
- `appMachine` (XState): conversations list, connection status, current conversation ID
- React Router: current route/slug
- Component state: draft text, UI toggles

The palette needs read access to `appMachine` context (for conversations) and the ability to trigger navigation (via `useNavigate`). Both are available at app root level.

## User Journey 1: Quick Conversation Switch (REQ-CP-001, REQ-CP-003, REQ-CP-005)

**Scenario:** User is in conversation A, wants to check conversation B, then return.

1. User presses `Cmd+P`
2. Palette opens with recent conversations listed (empty query = recents)
3. User types "bro" to filter for "browser-console-log-retrieval"
4. Results narrow to matching conversations with state indicators
5. User presses `↓` to select, then `Enter`
6. Palette closes, navigates to conversation B
7. User presses `Cmd+P` again
8. Types "A" or uses `↓` to find conversation A in recents
9. `Enter` to return

**State transitions:**
```
closed ──Cmd+P──▶ open:search:empty
                      │
                  type "bro"
                      ▼
               open:search:filtered
                      │
                  ↓ + Enter
                      ▼
                   closed (navigate)
```

## User Journey 2: Execute Action (REQ-CP-002, REQ-CP-004)

**Scenario:** User wants to create a new conversation without navigating away.

1. User presses `Cmd+P`
2. Types `>new` 
3. Results show actions: "New Conversation", "New Conversation in Directory..."
4. User selects "New Conversation"
5. Action triggers new conversation flow (bottom sheet on mobile, inline on desktop)

**State transitions:**
```
closed ──Cmd+P──▶ open:search:empty
                      │
                  type ">"
                      ▼
               open:action:empty
                      │
                  type "new"
                      ▼
               open:action:filtered
                      │
                    Enter
                      ▼
                   closed (execute action)
```

## Component Structure (REQ-CP-006, REQ-CP-007)

### Search Source Interface (REQ-CP-006)

```typescript
// Source interface for search mode extensibility
interface PaletteSource {
  id: string;
  category: string;  // "Conversations", "Files", etc.
  
  // Return items matching query (empty query = show defaults/recents)
  search(query: string): PaletteItem[];
  
  // Handle selection
  onSelect(item: PaletteItem): void;
}

interface PaletteItem {
  id: string;
  title: string;           // Primary display text
  subtitle?: string;       // Secondary info (path, state, etc.)
  icon?: React.ReactNode;  // State dot, file icon, etc.
  metadata?: unknown;      // Source-specific data
}

// Built-in source: Conversations
class ConversationSource implements PaletteSource {
  id = 'conversations';
  category = 'Conversations';
  
  constructor(private conversations: Conversation[]) {}
  
  search(query: string): PaletteItem[] {
    if (!query) {
      // Return recent conversations
      return this.conversations
        .slice(0, 10)
        .map(this.toItem);
    }
    return fuzzyMatch(this.conversations, query, c => c.slug)
      .map(this.toItem);
  }
  
  private toItem(conv: Conversation): PaletteItem {
    return {
      id: conv.id,
      title: conv.slug,
      subtitle: conv.cwd,
      icon: <StateIndicator state={conv.state} />,
    };
  }
  
  onSelect(item: PaletteItem) {
    navigate(`/c/${item.title}`);
  }
}
```

### Action Interface (REQ-CP-007)

```typescript
// Action interface for action mode - simple parameterless shortcuts
interface PaletteAction {
  id: string;
  title: string;           // Display name: "New Conversation"
  category?: string;       // Grouping: "Conversation", "Navigation"
  shortcut?: string;       // Keyboard hint: "Cmd+N"
  icon?: React.ReactNode;
  handler: () => void;     // Execute the action
}

// Built-in actions
const builtInActions: PaletteAction[] = [
  {
    id: 'new-conversation',
    title: 'New Conversation',
    category: 'Conversation',
    shortcut: 'n',
    handler: () => openNewConversationSheet(),
  },
  {
    id: 'archive-current',
    title: 'Archive Current Conversation',
    category: 'Conversation',
    handler: () => archiveCurrentConversation(),
  },
  {
    id: 'go-to-list',
    title: 'Go to Conversation List',
    category: 'Navigation',
    shortcut: 'Escape',
    handler: () => navigate('/'),
  },
];
```

## State Machine

### Why a State Machine?

The command palette has several interacting concerns—open/closed state, mode (search vs action), query text, selection index, and results—that could easily become tangled. Modeling as an explicit state machine provides:

1. **Exhaustive transition handling:** TypeScript's discriminated unions ensure every event is handled in every state. Adding a new event forces updates to all state handlers at compile time.

2. **Impossible states are unrepresentable:** A closed palette cannot have a selection index. By structuring state as `Closed | Open { mode, query, selectedIndex, results }`, we make invalid combinations impossible to construct.

3. **Testable transitions:** Pure `transition(state, event) → state` function enables property-based testing:
   - "ESCAPE from any Open state → Closed"
   - "SELECT_NEXT never exceeds results.length - 1"
   - "SET_QUERY starting with '>' always produces mode: 'action'"

4. **Predictable UI:** React component simply renders based on current state. No scattered `if` checks or derived booleans.

### State Definition

```typescript
// Closed state has no data
type ClosedState = { status: 'closed' };

// Open state carries all palette data
type OpenState = {
  status: 'open';
  mode: 'search' | 'action';
  query: string;           // Without the '>' prefix
  selectedIndex: number;   // Always valid: 0 <= idx < results.length (or 0 if empty)
  results: PaletteItem[];
};

type PaletteState = ClosedState | OpenState;

// Events
type PaletteEvent =
  | { type: 'OPEN' }
  | { type: 'CLOSE' }
  | { type: 'SET_QUERY'; query: string }
  | { type: 'SELECT_NEXT' }
  | { type: 'SELECT_PREV' }
  | { type: 'CONFIRM' };
```

### Invariants Enforced at Type Level

| Invariant | How Enforced |
|-----------|-------------|
| Closed palette has no query | `ClosedState` has no `query` field |
| Selection index in bounds | Transition function clamps index; type doesn't allow negative |
| Mode derived from query | `SET_QUERY` transition computes mode, not stored independently |
| Results match current query | Transition recomputes results on every `SET_QUERY` |

### Testing Patterns

```typescript
// Property: ESCAPE always closes
test.prop([arbitraryOpenState, fc.constant({ type: 'ESCAPE' })])(
  'ESCAPE from open state closes palette',
  (state, event) => {
    const next = transition(state, event);
    expect(next.status).toBe('closed');
  }
);

// Property: Selection stays in bounds
test.prop([arbitraryOpenState, fc.integer({ min: 0, max: 100 })])(
  'SELECT_NEXT never exceeds results length',
  (state, times) => {
    let current = state;
    for (let i = 0; i < times; i++) {
      current = transition(current, { type: 'SELECT_NEXT' });
    }
    if (current.status === 'open') {
      expect(current.selectedIndex).toBeLessThan(current.results.length);
    }
  }
);
```

## Keyboard Handling (REQ-CP-001, REQ-CP-005)

```typescript
// Global shortcut listener (at App root)
useEffect(() => {
  const handler = (e: KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key === 'p') {
      e.preventDefault();
      openPalette();
    }
  };
  window.addEventListener('keydown', handler);
  return () => window.removeEventListener('keydown', handler);
}, []);

// Palette-internal keyboard handling
const handleKeyDown = (e: React.KeyboardEvent) => {
  switch (e.key) {
    case 'ArrowDown':
    case 'n': // Ctrl+N
      if (e.key === 'n' && !e.ctrlKey) break;
      e.preventDefault();
      selectNext();
      break;
    case 'ArrowUp':
    case 'p': // Ctrl+P (when palette open, not the open shortcut)
      if (e.key === 'p' && !e.ctrlKey) break;
      e.preventDefault();
      selectPrev();
      break;
    case 'Enter':
      e.preventDefault();
      confirmSelection();
      break;
    case 'Escape':
      e.preventDefault();
      closePalette();
      break;
  }
};
```

## Styling (REQ-CP-007)

```css
.command-palette {
  position: fixed;
  z-index: 1000;
  
  /* Desktop: centered modal */
  @media (min-width: 768px) {
    top: 20%;
    left: 50%;
    transform: translateX(-50%);
    width: 600px;
    max-height: 400px;
    border-radius: 8px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
  }
  
  /* Mobile: full-width from top */
  @media (max-width: 767px) {
    top: 0;
    left: 0;
    right: 0;
    max-height: 70vh;
    border-radius: 0 0 8px 8px;
  }
}

.palette-result {
  min-height: 44px;  /* Touch-friendly */
  padding: 8px 12px;
  display: flex;
  align-items: center;
  gap: 8px;
}

.palette-result.selected {
  background: var(--color-accent-subtle);
}
```

## Integration Points

1. **Conversation list data:** Palette needs access to conversation list (from existing API/state)
2. **Navigation:** Uses react-router `useNavigate`
3. **Actions:** Need registry of available actions with handlers
4. **State indicators:** Reuses same visual language as conversation list (REQ-UI-012)

## Future Extensions

- **File search:** Add `FileSource` that searches project files (requires backend)
- **Settings:** Add `SettingsSource` for quick setting toggles
- **History:** Add `HistorySource` for recently executed actions
