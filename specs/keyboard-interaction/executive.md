# Keyboard Interaction Model - Executive Summary

## Requirements Summary

Phoenix IDE's keyboard interaction model defines how keyboard focus is scoped
across UI panels to prevent key conflicts. When an interactive panel (question
wizard, task approval, command palette) appears, it captures navigation keys
while global shortcuts (Ctrl+P / Cmd+P) pass through. Auto-focus ensures
keyboard interaction starts immediately when panels appear. A context-aware
help panel (`?` key) shows available shortcuts. Tooltip hints display shortcuts
on hover. The spec serves as a guardrail for coding agents building new
keyboard-interactive components.

## Technical Summary

Layered priority model using DOM event propagation. Each interactive panel calls
`stopPropagation` for keys it handles; unhandled events bubble to lower-priority
scopes. Lower-priority handlers (sidebar nav) check for active higher-priority
panels before handling events. Auto-focus uses `useEffect` with
`requestAnimationFrame` fallback. Escape propagates upward through the scope
stack -- the first handler that consumes it wins. Global shortcuts use modifier
keys and are never blocked by panel-level handlers.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-KB-001:** Layered Focus Scoping | ✅ Complete | `FocusScopeContext` in `useFocusScope.tsx` |
| **REQ-KB-002:** Global Shortcuts Pass Through | ✅ Complete | `useGlobalKeyboardShortcuts` checks `hasActiveScope` |
| **REQ-KB-003:** Scope-Local Key Consumption | ✅ Complete | QuestionPanel uses component-level `onKeyDown` with `stopPropagation` |
| **REQ-KB-004:** Auto-Focus on Panel Appearance | ✅ Complete | QuestionPanel auto-focuses first option on mount and step change |
| **REQ-KB-005:** Escape Key Behavior | ✅ Complete | QuestionPanel, ShortcutHelpPanel, ProseReader, TaskApprovalReader all register as focus scopes. Escape hierarchy: sub-context -> confirm if unsaved -> dismiss -> navigate |
| **REQ-KB-006:** Shortcut Help Panel | ✅ Complete | `?` key opens `ShortcutHelpPanel` |
| **REQ-KB-007:** Tooltip Shortcut Hints | ✅ Complete | Submit, Decline, Send buttons show shortcuts |
| **REQ-KB-008:** Prevent Key Leak to Inactive Scopes | ✅ Complete | `useKeyboardNav` gated by `hasActiveScope` |

**Progress:** 8 of 8 complete

## Cross-Spec References

- `specs/ask-user-question/` -- QuestionPanel keyboard behavior must comply
  with REQ-KB-001 through REQ-KB-005
- `specs/ui/` -- Sidebar navigation must respect REQ-KB-008
- `specs/command-palette/` -- Must comply with REQ-KB-001, REQ-KB-002
- `specs/inline-references/` -- Input area shortcuts reference REQ-KB-003
