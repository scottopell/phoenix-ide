# In-Viewer Find - Executive Summary

## Requirements Summary

In-viewer find gives Phoenix-owned `Cmd/Ctrl+F` search to the topmost eligible text surface instead of relying on the browser's DOM-only find. Users can open a compact find bar inside long files, diffs, task approval readers, and conversation-style text surfaces; type a literal query; see current/total results; step forward and backward with keyboard or buttons; and close find with Escape without dismissing the enclosing surface. The feature searches the surface's logical typed content, not just currently mounted DOM, so virtualized and off-screen matches remain reachable. Search state stays local to the owning surface, repeated `Cmd/Ctrl+F` refocuses the existing query, and editable controls keep their normal text-entry behavior unless the topmost surface explicitly owns the shortcut.

## Current Reality

Phoenix's shared viewer-find primitives live in `ui/src/components/viewer-find/`. `FindBar` provides accessible search chrome with autofocus, Enter/Shift+Enter navigation, and Escape-to-close behavior. `literalMatch` is the shared case-insensitive literal matcher. `findSession` is the shared closed/open state machine with branded match IDs, surface keys, focus origins, refocus sequencing, stable-ID reconciliation with nearest-prior fallback, and typed commands for query focus, focus restoration, match reveal, and decoration clearing.

File, diff, transcript, and task-approval surfaces consume the same typed session. Their canonical semantic projections carry stable source identity separately from mutable reveal coordinates. Pierre adapters reveal file and diff ranges; Virtuoso mounts transcript rows and expands renderer-owned fragments; task approval parses Markdown into stable display blocks shared by matching and rendering. The provider-owned keyboard router selects the topmost eligible modal, viewer, or passive-content registration independently of listener order.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-IVF-001:** Eligible Surface Ownership | ✅ Complete | `Cmd/Ctrl+F` is owned by the topmost eligible viewer scope; image/opaque viewers and unsupported preview contexts remain ineligible |
| **REQ-IVF-002:** Search the Logical Content, Not Just Mounted DOM | ✅ Complete | File, diff, transcript, tool, sub-agent, and parsed Markdown projections search canonical logical content independently of mount/disclosure state |
| **REQ-IVF-003:** Query Semantics and Result Ordering | ✅ Complete | `literalMatch.ts` remains the one case-insensitive literal matcher; empty-query=no-results and ordered matches remain the shared semantics |
| **REQ-IVF-004:** Match Counts and Navigation | ✅ Complete | Every eligible surface derives count, active ordinal, and wraparound navigation from the shared session match list |
| **REQ-IVF-005:** Navigation to Off-Screen Matches | ✅ Complete | Typed reveal commands scroll Pierre ranges, mount Virtuoso rows, and expand collapsed renderer-owned fragments before exact highlighting |
| **REQ-IVF-005A:** Every Result Has Stable Identity and Reveal Semantics | ✅ Complete | Every session match carries branded stable identity and an adapter-owned target; mutable line and row coordinates are reveal metadata rather than identity |
| **REQ-IVF-006:** Visible Match Indication | ✅ Complete | Active occurrence is distinguished from other rendered matches; fallback location indication is documented where exact substring styling is renderer-limited |
| **REQ-IVF-007:** Focus Lifecycle | ✅ Complete | Shared session commands preserve the original focus origin, refocus repeated shortcuts, and restore valid focus on close |
| **REQ-IVF-008:** Escape Closes Find Before the Enclosing Surface | ✅ Complete | Escape dismisses find without dismissing the enclosing viewer/approval surface |
| **REQ-IVF-009:** Scope-Local State and Overlay Isolation | ✅ Complete | Each surface owns one structural session and the central keyboard router prevents obscured lower layers from reacting |
| **REQ-IVF-010:** Editable Controls Keep Their Native Editing Behavior | ✅ Complete | Shortcut interception skips unrelated editable controls; the find input itself preserves text editing semantics |
| **REQ-IVF-011:** Result Reconciliation as Content Changes | ✅ Complete | All eligible surfaces preserve surviving active IDs and use deterministic nearest-prior fallback when content changes |
| **REQ-IVF-012:** Accessibility and Keyboard-Only Operation | ✅ Complete | Labelled search UI, keyboard navigation, status announcement, and focus-visible controls |

**Progress:** 13 complete, 0 partial, 0 missing

## Verification Coverage

- Unit coverage beside the shared primitives verifies literal matching, keyboard routing, structural closed/open state, stable-ID reconciliation, wraparound navigation, surface replacement, reset/close commands, and invalid-state type shape.
- Focus-scope and keyboard-router coverage verifies that hidden lower scopes cannot steal `Cmd/Ctrl+F`, editable controls retain native behavior, and repeated shortcuts return to the owning query.
- Integration coverage for files, diffs, task approval, and virtualized transcripts verifies off-screen navigation, exact renderer-owned highlighting, focus restoration, disclosure expansion, content-update reconciliation, surface swaps, and streaming isolation.

## Related Specs

- `specs/keyboard-interaction/` — topmost-scope ownership, autofocus, repeated shortcut refocus, and Escape hierarchy that in-viewer find instantiates
- `specs/prose-feedback/` — file viewer and task-approval reader surfaces that host in-viewer find
- `specs/viewer_slot/` — task approval remains a separate overlay lifecycle even when it hosts the shared find affordance
