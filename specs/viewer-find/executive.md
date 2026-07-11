# In-Viewer Find - Executive Summary

## Requirements Summary

In-viewer find gives Phoenix-owned `Cmd/Ctrl+F` search to the topmost eligible text surface instead of relying on the browser's DOM-only find. Users can open a compact find bar inside long files, diffs, task approval readers, and conversation-style text surfaces; type a literal query; see current/total results; step forward and backward with keyboard or buttons; and close find with Escape without dismissing the enclosing surface. The feature searches the surface's logical typed content, not just currently mounted DOM, so virtualized and off-screen matches remain reachable. Search state stays local to the owning surface, repeated `Cmd/Ctrl+F` refocuses the existing query, and editable controls keep their normal text-entry behavior unless the topmost surface explicitly owns the shortcut.

## Current Reality

Phoenix's shared viewer-find primitives live in `ui/src/components/viewer-find/`. `FindBar.tsx` provides the accessible search chrome with autofocus, Enter/Shift+Enter navigation, and Escape-to-close behavior. `useViewerFind.ts` owns local open/query/active-index state and recomputes case-insensitive literal matches through `literalMatch.ts`. `useViewerFindKeyboardShortcut.ts` integrates with `ui/src/hooks/useFocusScope.tsx` so a scope-owned `Cmd/Ctrl+F` only fires when that surface is the active scope and the event target is not an editable control that should keep native behavior.

The viewer stack registers the enclosing file viewer as a focus scope in `ui/src/components/viewer/MetaViewer.tsx`, which makes viewer-local shortcut ownership and Escape hierarchy compose with the broader keyboard model. Text-like file viewers and task-approval readers are the intended DOM-backed adopters of the shared find bar. Virtualized surfaces such as Pierre-backed file/diff views and transcript-style readers must search typed projections and navigate via stable typed targets rather than mounted DOM nodes; that behavior is part of the intended completed implementation this spec tracks.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-IVF-001:** Eligible Surface Ownership | ✅ Complete | `Cmd/Ctrl+F` is owned by the topmost eligible viewer scope; image/opaque viewers and unsupported preview contexts remain ineligible |
| **REQ-IVF-002:** Search the Logical Content, Not Just Mounted DOM | ✅ Complete | Search indexes derive from typed surface data so virtualization does not create phantom browser-find gaps |
| **REQ-IVF-003:** Query Semantics and Result Ordering | ✅ Complete | Case-insensitive literal matching, empty-query=no-results, document-order navigation |
| **REQ-IVF-004:** Match Counts and Navigation | ✅ Complete | Count/status plus Enter, Shift+Enter, Previous, and Next wraparound navigation |
| **REQ-IVF-005:** Navigation to Off-Screen Matches | ✅ Complete | Off-screen targets resolve through stable typed navigation handles rather than DOM identity |
| **REQ-IVF-006:** Visible Match Indication | ✅ Complete | Active occurrence is distinguished from other rendered matches; fallback location indication is documented where exact substring styling is renderer-limited |
| **REQ-IVF-007:** Focus Lifecycle | ✅ Complete | Opening find autofocuses/selects the query; repeated shortcut refocuses; closing restores prior in-surface focus |
| **REQ-IVF-008:** Escape Closes Find Before the Enclosing Surface | ✅ Complete | Escape dismisses find without dismissing the enclosing viewer/approval surface |
| **REQ-IVF-009:** Scope-Local State and Overlay Isolation | ✅ Complete | Find state is surface-local and hidden lower scopes do not react while a higher overlay is active |
| **REQ-IVF-010:** Editable Controls Keep Their Native Editing Behavior | ✅ Complete | Shortcut interception skips unrelated editable controls; the find input itself preserves text editing semantics |
| **REQ-IVF-011:** Result Reconciliation as Content Changes | ✅ Complete | Live content updates recompute results while preserving the active logical occurrence when still present |
| **REQ-IVF-012:** Accessibility and Keyboard-Only Operation | ✅ Complete | Labelled search UI, keyboard navigation, status announcement, and focus-visible controls |

**Progress:** 12 of 12 complete

## Verification Coverage

- Unit coverage for literal matching, active-index reconciliation, and keyboard routing lives beside the shared primitives in `ui/src/components/viewer-find/`.
- Focus-scope ownership is covered by the `useFocusScope` and viewer-find shortcut tests so hidden lower scopes cannot steal `Cmd/Ctrl+F`.
- End-to-end coverage for files, diffs, task approval, and transcript-style surfaces must verify off-screen navigation, highlighting, repeated shortcut refocus, editable-target passthrough, and Escape closing only the find bar.

## Related Specs

- `specs/keyboard-interaction/` — topmost-scope ownership, autofocus, repeated shortcut refocus, and Escape hierarchy that in-viewer find instantiates
- `specs/prose-feedback/` — file viewer and task-approval reader surfaces that host in-viewer find
- `specs/viewer_slot/` — task approval remains a separate overlay lifecycle even when it hosts the shared find affordance
