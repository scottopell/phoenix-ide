# Command Palette

## User Story

As a power user, I need a single keyboard-driven interface to quickly navigate conversations, execute actions, and search across the app so that I can stay in flow without reaching for the mouse or navigating through menus.

## Requirements

### REQ-CP-001: Single Global Shortcut

WHEN user presses `Ctrl+P` (or `Cmd+P` on macOS)
THE SYSTEM SHALL open the command palette overlay
AND focus the input field immediately

WHEN command palette is open and user presses `Escape`
THE SYSTEM SHALL close the palette and return focus to previous context

WHEN command palette is open and user clicks outside
THE SYSTEM SHALL close the palette

**Rationale:** A single, memorable shortcut reduces cognitive load. Users learn one pattern that works everywhere in the app.

---

### REQ-CP-002: Prefix-Based Mode Switching

WHEN user types input starting with `>`
THE SYSTEM SHALL enter "action mode" and show only executable commands
AND strip the `>` prefix from the filter query

WHEN user types input NOT starting with `>`
THE SYSTEM SHALL enter "search mode" and show searchable items (conversations, etc.)

WHEN user clears the `>` prefix from action mode
THE SYSTEM SHALL switch back to search mode immediately

**Rationale:** Single input field serves multiple purposes without mode buttons or tabs. Users who know what they want can prefix; others discover via results.

---

### REQ-CP-003: Search Mode Behavior

WHEN in search mode with a query
THE SYSTEM SHALL query all registered search sources
AND filter results by fuzzy match on display text
AND rank results by match quality and source-specific criteria (e.g., recency)
AND group results by source category

WHEN user selects a search result
THE SYSTEM SHALL invoke the source's selection handler
AND close the palette

WHEN no query is entered (empty search mode)
THE SYSTEM SHALL show default results from each source (e.g., recent conversations)

**Rationale:** Search mode is the default experience. Multiple sources can contribute results, enabling future expansion (files, bookmarks) without changing core behavior.

---

### REQ-CP-004: Action Mode Behavior

WHEN in action mode with a query
THE SYSTEM SHALL query all registered actions
AND filter actions by fuzzy match on action name
AND show keyboard shortcut hints where applicable

WHEN user selects an action
THE SYSTEM SHALL execute the action's handler
AND close the palette

WHEN no query is entered (empty action mode)
THE SYSTEM SHALL show all available actions grouped by category

**Rationale:** Action mode mirrors search mode structure but for executable commands. Consistent UX across both modes reduces learning curve.

---

### REQ-CP-005: Keyboard Navigation

WHEN command palette is open
THE SYSTEM SHALL support `↑`/`↓` (or `Ctrl+P`/`Ctrl+N`) to navigate results
AND support `Enter` to select highlighted result
AND support `Tab` to autocomplete partial matches

WHEN navigating results
THE SYSTEM SHALL keep the input field focused
AND visually highlight the selected result

**Rationale:** Full keyboard operation enables flow state without mouse context-switching.

---

### REQ-CP-006: Extensible Source Interface

WHEN registering a new searchable source
THE SYSTEM SHALL accept a source that provides:
- A list of searchable items with display text and metadata
- A handler for when an item is selected
- Optional: icon, category label, keyboard shortcut

WHEN multiple sources are registered
THE SYSTEM SHALL merge results and group by source category

**Rationale:** Future sources (project files, bookmarks, settings) can plug into the same interface without palette changes.

---

### REQ-CP-007: Extensible Action Interface

WHEN registering a new action
THE SYSTEM SHALL accept an action that provides:
- A unique identifier and display name
- A handler function to execute when selected
- Optional: keyboard shortcut hint, category label, icon

WHEN multiple actions are registered
THE SYSTEM SHALL make all actions available in action mode
AND support filtering across all registered actions

**Rationale:** Actions are the command palette's power-user capability. A well-defined interface enables adding new actions (archive, rename, settings toggles) without modifying palette internals.

---

### REQ-CP-008: Desktop-Only Initial Scope

WHEN viewport is mobile-sized
THE SYSTEM SHALL NOT render the command palette
AND SHALL NOT register keyboard shortcuts

WHEN viewport is desktop-sized
THE SYSTEM SHALL render the palette as a centered modal (approximately 600px wide)
AND show results without requiring scroll for common cases (8-10 items visible)

**Rationale:** Mobile lacks keyboard shortcuts to trigger the palette. Rather than inventing touch-based triggers, we scope to desktop initially and revisit mobile integration when use cases are clearer.
