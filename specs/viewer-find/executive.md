# In-Viewer Find - Executive Summary

## Requirements Summary

In-viewer find gives Phoenix-owned `Cmd/Ctrl+F` search to the topmost eligible text surface instead of relying on the browser's DOM-only find. Users can open a compact find bar inside long files, diffs, task approval readers, and conversation-style text surfaces; type a literal query; see current/total results; step forward and backward with keyboard or buttons; and close find with Escape without dismissing the enclosing surface. The feature searches the surface's logical typed content, not just currently mounted DOM, so virtualized and off-screen matches remain reachable. Search state stays local to the owning surface, repeated `Cmd/Ctrl+F` refocuses the existing query, and editable controls keep their normal text-entry behavior unless the topmost surface explicitly owns the shortcut.

## Current Reality

Phoenix's shared viewer-find primitives live in `ui/src/components/viewer-find/`. `FindBar` provides accessible search chrome with autofocus, Enter/Shift+Enter navigation, and Escape-to-close behavior. `literalMatch` is the shared case-insensitive literal matcher. `findSession` is the shared closed/open state machine with branded match IDs, surface keys, focus origins, refocus sequencing, stable-ID reconciliation with nearest-prior fallback, and typed commands for query focus, focus restoration, match reveal, and decoration clearing.

File, diff, transcript, and task-approval surfaces consume the same typed session. Their canonical semantic projections carry stable source identity separately from mutable reveal coordinates. Pierre adapters reveal file and diff ranges; Virtuoso mounts transcript rows and expands renderer-owned fragments; task approval parses Markdown into stable display blocks shared by matching and rendering. The provider-owned keyboard router selects the topmost eligible modal, viewer, or passive-content registration independently of listener order.

### Conversation Refresh and Reveal

The separate Find fix uses `refresh-results` for conversation projection updates: a retained logical active ID receives fresh physical targets without a new reveal command. Other consumers' `replace-results` behavior remains unchanged. Query changes, Next/Previous, explicit activation, and active-occurrence removal still issue reveals; no results clear decorations. `MessageList` separates selection decoration from reveal intent. Pending reveal retries read the latest target rather than capturing stale streaming coordinates; a later selection, close, or surface reset cancels prior work. Settled navigation is not restarted by a background refresh.

`conversationProjectionSessionMatches` bridges streaming raw offsets to finalized semantic offsets only for the same request/turn with exactly one text block and a uniquely contiguous display-block mapping. The text block may follow non-text content. Covered prose includes plain paragraphs, headings, simple emphasis, and quotes. Multiple text blocks, changed raw offsets, ambiguous/noncontiguous text, cross-display-block matches, and separate code-fragment projections do not receive this correspondence. Blocks containing link/image syntax, HTML, escapes, entities, or backticks are conservatively excluded, including hidden destinations, alt text, and titles that could otherwise impersonate visible text provenance. These exclusions restrict identity transfer, not ordinary search eligibility; unmatched identities use normal fallback reconciliation. This is not a general Markdown source map or a guarantee for arbitrary streaming-to-final transformations.

Explicit conversation navigation prefers the renderer's exact `[data-viewer-find-active-target]` mark inside the selected fragment. Centering a large fragment alone does not establish that its active occurrence is visible. When no exact mark exists, navigation falls back to the fragment/row and its active-location indication. Streaming rows use row-level indication rather than an exact inline active mark.

## Status Summary

The table describes implementation coverage, not release or production deployment. The separate conversation refresh/reveal fix is local work; hosted verification is pending, and shipping is not claimed.

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
| **REQ-IVF-011:** Result Reconciliation as Content Changes | ✅ Implemented locally; hosted verification pending for the separate Find fix | Conversation refresh retains surviving IDs without new reveal; pending explicit intent uses fresh targets. Bounded streaming/final correspondence and deterministic removal fallback are covered below |
| **REQ-IVF-012:** Accessibility and Keyboard-Only Operation | ✅ Complete | Labelled search UI, keyboard navigation, status announcement, and focus-visible controls |

**Implementation coverage:** 13 requirements represented. This is not a claim that the separate Find fix has shipped.

## Verification Coverage

- Unit coverage beside the shared primitives verifies literal matching, keyboard routing, structural closed/open state, stable-ID reconciliation, wraparound navigation, surface replacement, reset/close commands, and invalid-state type shape.
- Focus-scope and keyboard-router coverage verifies that hidden lower scopes cannot steal `Cmd/Ctrl+F`, editable controls retain native behavior, and repeated shortcuts return to the owning query.
- Integration coverage for files, diffs, task approval, and virtualized transcripts verifies off-screen navigation, exact renderer-owned highlighting, focus restoration, disclosure expansion, content-update reconciliation, surface swaps, and streaming isolation.

### Separate Find Fix: Local Verification Scope

- `findSession.test.ts` covers silent retained-ID refresh with updated targets, unchanged explicit replacement behavior, Next/Previous and activation using the refreshed target, query reset, removal fallback, no-results decoration clearing, and closed/reset isolation.
- `conversationProjectionSession.test.ts` covers supported prose identity transfer, nonzero text-block index, uniqueness, request separation, changed offsets/removal, multiple-text rejection, and hidden Markdown destination/alt/title provenance exclusions.
- `MessageList.test.tsx` covers settled versus pending finalization, unrelated refresh during pending historical navigation, superseding selection, close/reset cancellation, latest-fragment reveal, and preferring the exact active mark over its larger fragment. These tests use mocked transcript/renderers; they are not browser layout evidence.
- The local-alias browser scenario `find-identical-finalization` is wired through `capture-message-list.mjs` and `message-list-find-finalization.mjs`. Its four controls are streaming-active with wheel movement, historical-active, closed Find, and streaming-active without wheel movement. After draining reveal work, it samples animation frames across identical-text finalization and checks no scroll/focus commands, stable visible reading coordinates within 2px, retained selection/active owner, and subsequent Next/Previous revealing the active mark within the viewport. The fixture uses identical plain prose; it does not prove arbitrary Markdown finalization or pending-navigation layout behavior.
- These are local-alias browser and local test coverage, not hosted or production evidence. Hosted verification for this separate fix remains pending. No run result or deployment is inferred from the presence of the tests and capture script.

## Related Specs

- `specs/keyboard-interaction/` — topmost-scope ownership, autofocus, repeated shortcut refocus, and Escape hierarchy that in-viewer find instantiates
- `specs/prose-feedback/` — file viewer and task-approval reader surfaces that host in-viewer find
- `specs/viewer_slot/` — task approval remains a separate overlay lifecycle even when it hosts the shared find affordance
