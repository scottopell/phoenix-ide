# Keyboard Interaction Model - Technical Design

## Architecture Overview

Keyboard interaction uses a layered priority model built on DOM event
propagation. Each interactive panel registers a keydown handler that calls
`event.stopPropagation()` for keys it handles. Lower-priority handlers only
fire if the event was not consumed by a higher-priority scope.

The focus scope stack is implicit in the DOM hierarchy: panels rendered later
(higher in z-order) have handlers that fire first in the capture phase, or
use `stopPropagation` in the bubble phase to prevent lower handlers from
seeing the event.

## Focus Scope Stack (REQ-KB-001, REQ-KB-003, REQ-KB-008)

### Scope hierarchy (highest to lowest priority)

1. **Modal dialogs** (ConfirmDialog, first-task welcome) -- captures all keys
2. **Interactive panels** (QuestionPanel, TaskApprovalReader, command palette)
3. **Input area** (message textarea, slash commands, bang commands)
4. **Page-level navigation** (sidebar keyboard nav, Escape to go home)
5. **Global shortcuts** (Ctrl+P, Ctrl+K, `?`) -- always active

### Implementation pattern

Every component that handles keyboard events must follow this contract:

1. Register a `keydown` handler on the component's root element (not
   `document` or `window`)
2. For keys the component handles: call `event.stopPropagation()` and
   `event.preventDefault()`
3. For keys the component does not handle: do nothing (let the event bubble)
4. Check `event.target` to avoid capturing events from nested inputs

Components that currently register on `window` or `document` (violating this
pattern):
- `useGlobalKeyboardShortcuts` -- registers on `window` (scope 5, acceptable
  for global shortcuts)
- `useKeyboardNav` -- registers on `window` (scope 4, needs gating)
- `QuestionPanel` -- registers on `document` (scope 2, needs migration to
  component-level)

### Gating lower-priority handlers (REQ-KB-008)

`useKeyboardNav` (sidebar navigation) must check whether a higher-priority
scope is active before handling events. The check:

```
// In useKeyboardNav's handler:
// If any interactive panel is mounted, suppress sidebar nav keys
if (document.querySelector(
  '.question-panel, .task-approval-reader, [role="dialog"], .command-palette'
)) {
  return; // higher-priority scope is active
}
```

This is a pragmatic approach. A more formal approach would use a React context
that tracks the active scope stack, but the DOM query is simpler and doesn't
require threading context through the component tree.

## Auto-Focus (REQ-KB-004)

When an interactive panel mounts, it must focus its primary element:

- **QuestionPanel**: first option of the current question (or pre-selected
  option)
- **TaskApprovalReader**: the plan text area or first action button
- **Command palette**: the search input
- **ConfirmDialog**: the confirm or cancel button

Use `useEffect` with `ref.current?.focus()` on mount. If the element isn't
rendered yet (conditional rendering), use `requestAnimationFrame` to retry.

When a panel unmounts, restore focus to the previously focused element. Capture
`document.activeElement` before mounting the panel and restore it in the
cleanup function.

## Escape Key Hierarchy (REQ-KB-005)

Escape propagates upward through the scope stack. The first scope that handles
it consumes it:

1. ConfirmDialog open -> dismiss dialog
2. QuestionPanel active (with unsaved answers) -> show confirm dialog
3. QuestionPanel active (no answers) -> dismiss panel (decline)
4. Input area focused -> blur input
5. Conversation page, no panels -> navigate to list
6. List page -> no-op

The current `useGlobalKeyboardShortcuts` handles Escape at scope 5 (navigate
to list). It must check whether a higher-priority scope consumed the event
before navigating. The DOM query approach from the gating section applies here
too.

## Global Shortcuts (REQ-KB-002)

Global shortcuts use modifier keys (Ctrl/Cmd) and are handled at the lowest
priority but are not blocked by higher scopes because:

1. Higher scopes only call `stopPropagation` for navigation keys (arrows, Tab,
   Enter, Space, Escape)
2. Modifier-key combinations (Ctrl+P, Ctrl+K) are explicitly NOT consumed by
   panel-level handlers
3. The `?` key (no modifier) is global but must check that the user is not
   typing in a text input

## Help Panel (REQ-KB-006)

A modal overlay triggered by `?` that shows context-aware shortcuts.

### Data model

```typescript
interface ShortcutEntry {
  key: string;        // "ArrowDown", "Ctrl+Enter", "?"
  description: string; // "Move to next option"
  scope: string;       // "Question Panel", "Global", "Navigation"
}
```

Each component exports a `getShortcuts(): ShortcutEntry[]` function. The help
panel collects shortcuts from the active scope stack and renders them grouped
by scope.

### Layout

- Modal overlay (like GitHub's `?` panel)
- Three columns: Key | Description | Scope
- Grouped by scope with headers
- Dismissed by Escape or `?` again

## Tooltip Hints (REQ-KB-007)

Buttons with keyboard shortcuts include the shortcut in their `title`
attribute:

- `Submit (Ctrl+Enter)`
- `Decline (Escape)`
- `Next question (Tab)`

Format shortcuts using the platform: `Cmd` on macOS, `Ctrl` elsewhere. Detect
via `navigator.platform` or `navigator.userAgent`.

## Per-Scope Key Bindings

### QuestionPanel (cross-references specs/ask-user-question)

| Key | Single-select | Multi-select |
|-----|--------------|-------------|
| ArrowUp/Down | Move focus ring | Move focus ring |
| Enter | Select focused option | Toggle focused option |
| Space | Select focused option | Toggle focused option |
| Tab | Advance to next question | Advance to next question |
| Shift+Tab | Go to previous question | Go to previous question |
| n | Open notes (preview questions only) | -- |
| Ctrl+Enter | Submit all answers | Submit all answers |
| Escape | Confirm decline dialog | Confirm decline dialog |

### Sidebar Navigation (cross-references specs/ui)

| Key | Action |
|-----|--------|
| ArrowUp/Down | Move between conversations |
| Enter | Open selected conversation |
| n | New conversation |

### Input Area (cross-references specs/inline-references)

| Key | Action |
|-----|--------|
| Enter | Send message (when not Shift+Enter) |
| Shift+Enter | Newline |
| / | Trigger slash command menu |
| ! | Trigger bash command prefix |
| Escape | Blur input |

### Global

| Key | Action |
|-----|--------|
| Ctrl+P / Cmd+P | Open command palette |
| ? | Open shortcut help panel |
| Escape | Close active panel / navigate back |

## Testing Strategy

### Unit Tests
- Scope stack: verify that mounting a QuestionPanel prevents sidebar nav
  from receiving arrow keys
- Auto-focus: verify that the primary element is focused within 100ms of mount
- Escape hierarchy: verify each level of the Escape chain fires correctly

### Integration Tests
- Full flow: open conversation -> agent asks question -> arrow keys only
  affect QuestionPanel, not sidebar -> submit -> sidebar nav resumes
- Help panel: `?` opens panel with current context shortcuts, `?` closes it
