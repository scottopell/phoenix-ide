# Keyboard Interaction Model - Executive Summary

## Requirements Summary

Phoenix IDE's keyboard interaction model defines how keyboard focus is scoped
across UI panels to prevent key conflicts. When an interactive panel (question
wizard, task approval, command palette, viewer-local find) appears, it captures
navigation keys while global shortcuts (Ctrl+P / Cmd+P) pass through.
Scope-owned shortcuts belong to the topmost eligible surface, not a hidden
lower layer, and repeated shortcuts refocus the already-open affordance instead
of spawning duplicates. Auto-focus ensures keyboard interaction starts
immediately when panels appear. A context-aware help panel (`?` key) shows
available shortcuts. Tooltip hints display shortcuts on hover. The spec serves
as a guardrail for coding agents building new keyboard-interactive components.

## Technical Summary

Layered priority model using DOM event propagation and the `FocusScopeContext`
stack. Each interactive panel calls `stopPropagation` for keys it handles;
unhandled events bubble to lower-priority scopes. Lower-priority handlers
(sidebar nav, viewer-local shortcuts) check the active higher-priority scope
before handling events. Auto-focus uses `useEffect` with
`requestAnimationFrame` fallback where needed. Escape propagates upward through
the scope stack -- the first handler that consumes it wins, so a topmost
sub-context such as in-viewer find closes before its enclosing viewer. Global
shortcuts use modifier keys and are never blocked by panel-level handlers.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-KB-001:** Layered Focus Scoping | ✅ Complete | `FocusScopeContext` in `ui/src/hooks/useFocusScope.tsx` manages push/pop ordering for topmost scopes |
| **REQ-KB-002:** Global Shortcuts Pass Through | ✅ Complete | `useGlobalKeyboardShortcuts` keeps global commands available across scope-local handlers |
| **REQ-KB-002A:** Topmost Eligible Shortcut Ownership | ✅ Complete | Viewer-local `Cmd/Ctrl+F` routing checks `activeScope` and ignores obscured lower layers via `useViewerFindKeyboardShortcut` |
| **REQ-KB-003:** Scope-Local Key Consumption | ✅ Complete | Panel-local `onKeyDown` handlers consume owned keys so lower scopes do not react |
| **REQ-KB-004:** Auto-Focus on Panel Appearance | ✅ Complete | Question-panel and viewer-find affordances move focus into their primary control on open |
| **REQ-KB-004A:** Repeated Shortcut Refocus | ✅ Complete | Re-pressing `Cmd/Ctrl+F` refocuses/selects the existing find query instead of opening another bar |
| **REQ-KB-005:** Escape Key Behavior | ✅ Complete | Escape hierarchy remains sub-context -> confirm if unsaved -> dismiss -> navigate; viewer find closes before its enclosing viewer |
| **REQ-KB-006:** Shortcut Help Panel | ✅ Complete | `?` key opens `ShortcutHelpPanel` |
| **REQ-KB-007:** Tooltip Shortcut Hints | ✅ Complete | Shortcut-bearing controls expose their key hints in tooltips or labels |
| **REQ-KB-008:** Prevent Key Leak to Inactive Scopes | ✅ Complete | Lower-priority keyboard navigation stays gated behind the active scope |

**Progress:** 10 of 10 complete

## Cross-Spec References

- `specs/ask-user-question/` -- QuestionPanel keyboard behavior must comply
  with REQ-KB-001 through REQ-KB-005
- `specs/conversation-ui/` -- Sidebar navigation must respect REQ-KB-008
- `specs/command-palette/` -- Must comply with REQ-KB-001, REQ-KB-002
- `specs/inline-references/` -- Input area shortcuts reference REQ-KB-003
- `specs/viewer-find/` -- In-viewer find owns `Cmd/Ctrl+F`, local focus restoration, and Escape behavior within the topmost eligible viewer scope
