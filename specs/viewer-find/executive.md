# In-Viewer Find - Executive Summary

## Requirements Summary

In-viewer find gives Phoenix-owned `Cmd/Ctrl+F` search to the topmost eligible text surface instead of relying on the browser's DOM-only find. Users can open a compact find bar inside long files, diffs, task approval readers, and conversation-style text surfaces; type a literal query; see current/total results; step forward and backward with keyboard or buttons; and close find with Escape without dismissing the enclosing surface. The feature searches the surface's logical typed content, not just currently mounted DOM, so virtualized and off-screen matches remain reachable. Search state stays local to the owning surface, repeated `Cmd/Ctrl+F` refocuses the existing query, and editable controls keep their normal text-entry behavior unless the topmost surface explicitly owns the shortcut.

## Current Reality

Phoenix's shared viewer-find primitives live in `ui/src/components/viewer-find/`. `FindBar.tsx` provides the accessible search chrome with autofocus, Enter/Shift+Enter navigation, and Escape-to-close behavior. `literalMatch.ts` remains the one shared case-insensitive literal matcher consumed by the existing projection helpers. `findSession.ts` now provides the first generic pure find-session foundation: a closed/open discriminated state machine with branded match IDs, surface keys, focus-origin tokens, focus-version refocus sequencing, active-match reconciliation by stable ID then nearest prior ordinal, and typed emitted commands for query focus, focus restoration, match reveal, and decoration clearing. `useViewerFind.ts` still owns the legacy hook-level open/query/active-index state for current adopters.

The viewer stack registers the enclosing file viewer as a focus scope in `ui/src/components/viewer/MetaViewer.tsx`, which makes viewer-local shortcut ownership and Escape hierarchy compose with the broader keyboard model. Text-like file viewers and task-approval readers are the intended DOM-backed adopters of the shared find bar. Virtualized surfaces such as Pierre-backed file/diff views and transcript-style readers must search typed projections and navigate via stable typed targets rather than mounted DOM nodes. Those surfaces have not yet migrated to the generic find-session foundation; this executive tracks that foundation as shipped and the cross-surface migration as remaining work.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-IVF-001:** Eligible Surface Ownership | ✅ Complete | `Cmd/Ctrl+F` is owned by the topmost eligible viewer scope; image/opaque viewers and unsupported preview contexts remain ineligible |
| **REQ-IVF-002:** Search the Logical Content, Not Just Mounted DOM | 🟡 Partial | Shared projections and the new pure session foundation assume one canonical semantic-content search source, but not every surface has migrated to the unified typed session yet |
| **REQ-IVF-003:** Query Semantics and Result Ordering | ✅ Complete | `literalMatch.ts` remains the one case-insensitive literal matcher; empty-query=no-results and ordered matches remain the shared semantics |
| **REQ-IVF-004:** Match Counts and Navigation | 🟡 Partial | Current adopters expose counts and wraparound navigation; the new generic session now models next/previous/activate and emitted reveal commands but is not yet wired into every surface |
| **REQ-IVF-005:** Navigation to Off-Screen Matches | 🟡 Partial | Typed projections already navigate off-screen content, and the new session emits typed reveal commands, but the cross-surface session/reveal contract is not yet universal |
| **REQ-IVF-005A:** Every Result Has Stable Identity and Reveal Semantics | 🟡 Partial | `findSession.ts` introduces branded match IDs, surface keys, and typed reveal commands; existing surfaces still need migration from legacy local state to that shared contract |
| **REQ-IVF-006:** Visible Match Indication | ✅ Complete | Active occurrence is distinguished from other rendered matches; fallback location indication is documented where exact substring styling is renderer-limited |
| **REQ-IVF-007:** Focus Lifecycle | 🟡 Partial | The pure session now retains focus-origin tokens and refocus versions without replacing origin on reopen; current UI adopters still restore focus through legacy hook wiring |
| **REQ-IVF-008:** Escape Closes Find Before the Enclosing Surface | ✅ Complete | Escape dismisses find without dismissing the enclosing viewer/approval surface |
| **REQ-IVF-009:** Scope-Local State and Overlay Isolation | 🟡 Partial | Scope ownership is enforced today, and the new session can close/reset structurally, but one shared session model does not yet own every eligible surface |
| **REQ-IVF-010:** Editable Controls Keep Their Native Editing Behavior | ✅ Complete | Shortcut interception skips unrelated editable controls; the find input itself preserves text editing semantics |
| **REQ-IVF-011:** Result Reconciliation as Content Changes | 🟡 Partial | `findSession.ts` preserves active identity by stable match ID, then nearest prior ordinal when removed; surfaces still need migration to consume that shared reconciliation path |
| **REQ-IVF-012:** Accessibility and Keyboard-Only Operation | ✅ Complete | Labelled search UI, keyboard navigation, status announcement, and focus-visible controls |

**Progress:** 7 complete, 6 partial, 0 missing

## Verification Coverage

- Unit coverage for literal matching, keyboard routing, the new pure `findSession.ts` closed/open state machine, stable-ID reconciliation, wraparound navigation, reset/close effects, and invalid-state type shape lives beside the shared primitives in `ui/src/components/viewer-find/`.
- Focus-scope ownership is covered by the `useFocusScope` and viewer-find shortcut tests so hidden lower scopes cannot steal `Cmd/Ctrl+F`.
- End-to-end coverage for files, diffs, task approval, and transcript-style surfaces must verify off-screen navigation, highlighting, repeated shortcut refocus, editable-target passthrough, Escape closing only the find bar, and migration onto the shared typed session contract.

## Related Specs

- `specs/keyboard-interaction/` — topmost-scope ownership, autofocus, repeated shortcut refocus, and Escape hierarchy that in-viewer find instantiates
- `specs/prose-feedback/` — file viewer and task-approval reader surfaces that host in-viewer find
- `specs/viewer_slot/` — task approval remains a separate overlay lifecycle even when it hosts the shared find affordance
