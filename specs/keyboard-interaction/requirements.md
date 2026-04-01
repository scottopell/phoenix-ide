# Keyboard Interaction Model

## User Story

As a developer using Phoenix IDE, I need keyboard shortcuts to work predictably
across all UI states so that I can stay in flow without reaching for the mouse
or being surprised by unexpected navigation.

As a coding agent building new Phoenix features, I need a documented keyboard
scoping model so that I can add keyboard interactions to new components without
creating conflicts with existing shortcuts.

## Requirements

### REQ-KB-001: Layered Focus Scoping

THE SYSTEM SHALL maintain a stack of keyboard focus scopes where the topmost
scope captures navigation keys (arrows, Tab, Enter, Space, Escape)
AND lower scopes receive only events that the topmost scope does not consume

WHEN a new interactive panel appears (QuestionPanel, task approval, command
palette, modal dialog)
THE SYSTEM SHALL push a focus scope onto the stack
AND auto-focus the primary interactive element of that scope

WHEN the interactive panel is dismissed
THE SYSTEM SHALL pop the focus scope
AND restore focus to the element that was focused before the scope was pushed

**Rationale:** Developers expect that keyboard input goes to the thing they're
interacting with. When arrow keys in a question panel also scroll the sidebar,
the experience feels broken and unpredictable. Focus scoping prevents this
class of bug structurally.

**Dependencies:** All specs with keyboard behavior must reference this spec's
scoping rules.

---

### REQ-KB-002: Global Shortcuts Pass Through All Scopes

WHILE any focus scope is active
THE SYSTEM SHALL allow global shortcuts to pass through without being consumed
AND global shortcuts include: Ctrl+P / Cmd+P (command palette), `?` (help
panel)

IF a global shortcut conflicts with a scope-local key
THE SYSTEM SHALL give priority to the global shortcut when a modifier key
(Ctrl, Cmd, Alt) is held

**Rationale:** Developers expect app-level commands to work regardless of what
panel is open. Having to dismiss a panel to open the command palette breaks
flow.

---

### REQ-KB-003: Scope-Local Key Consumption

WHEN a focus scope consumes a key event
THE SYSTEM SHALL prevent that event from reaching lower scopes

WHEN a focus scope does not handle a key event
THE SYSTEM SHALL allow the event to propagate to the next scope in the stack

**Rationale:** This is the core conflict prevention mechanism. A QuestionPanel
that handles ArrowDown must prevent the sidebar from also handling ArrowDown.
The scope either consumes the event or passes it through -- there is no
ambiguity.

---

### REQ-KB-004: Auto-Focus on Panel Appearance

WHEN an interactive panel appears that accepts keyboard input
THE SYSTEM SHALL focus the primary interactive element within 100ms
AND the user SHALL be able to begin keyboard interaction without clicking

WHEN focus cannot be set (element not yet rendered)
THE SYSTEM SHALL retry focus on the next animation frame

**Rationale:** If the user has to click into a panel before keyboard works, the
keyboard flow never starts. Auto-focus is the critical gate that determines
whether keyboard navigation gets used at all.

---

### REQ-KB-005: Escape Key Behavior

WHEN Escape is pressed and an interactive panel is the topmost focus scope
THE SYSTEM SHALL dismiss or close that panel (with confirmation if the panel
has unsaved state)

WHEN Escape is pressed and the topmost scope has a sub-context (e.g.,
ProseReader with an open annotation input, or QuestionPanel with an open
notes field)
THE SYSTEM SHALL close the sub-context first, not the panel itself

WHEN Escape is pressed and no interactive panel is active
AND the user is on a conversation page
THE SYSTEM SHALL navigate to the conversation list

WHEN Escape is pressed and the user is in a text input
THE SYSTEM SHALL blur the input without navigating

IF Escape would dismiss a panel with unsaved user input (unanswered questions,
in-progress annotations, draft prose feedback)
THE SYSTEM SHALL show a confirmation dialog before dismissing

**Rationale:** Escape is the universal "back out" key. Its behavior must be
predictable: it always closes the nearest thing. Developers lose trust in
keyboard shortcuts when Escape does something unexpected (like navigating away
from a conversation while a prose reader has unsaved annotations).

---

### REQ-KB-006: Shortcut Help Panel

WHEN user presses `?` (and is not typing in a text input)
THE SYSTEM SHALL display a panel listing all keyboard shortcuts
AND group shortcuts by scope (global, navigation, panels)
AND dismiss the panel on Escape or `?` again

**Rationale:** Developers discover shortcuts by trying them or reading
documentation. A help panel (like GitHub's `?` modal) bridges the gap -- it
shows what's available without requiring the user to read a spec.

---

### REQ-KB-007: Tooltip Shortcut Hints

WHERE a button or action has a keyboard shortcut
THE SYSTEM SHALL display the shortcut in the button's tooltip
AND format shortcuts using platform conventions (Ctrl on Linux/Windows,
Cmd on macOS)

**Rationale:** Tooltips are the lowest-friction way to learn shortcuts. A
developer hovering over "Submit" sees "(Ctrl+Enter)" and immediately knows.
No separate documentation lookup needed.

---

### REQ-KB-008: Prevent Key Leak to Inactive Scopes

IF an interactive panel is the topmost focus scope
THE SYSTEM SHALL NOT allow navigation keys (arrows, Tab, Enter, Space) to
affect components outside that scope

**Rationale:** Key leak is the specific bug that triggered this spec. Arrow
keys in the QuestionPanel also navigated the sidebar conversation list. When
REQ-KB-001 and REQ-KB-003 are correctly implemented, key leak is structurally
impossible -- this requirement exists as a testable statement of that property.
