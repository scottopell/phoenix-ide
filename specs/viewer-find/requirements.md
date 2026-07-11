# In-Viewer Find

## User Story

As a developer reading long files, diffs, task plans, and conversation history in Phoenix, I need `Cmd/Ctrl+F` to search the content Phoenix is rendering rather than the browser's mounted DOM so I can reliably find and navigate matches even when virtualization or overlays hide part of the content.

As a coding agent extending Phoenix viewers, I need one documented in-app find model so that every eligible text surface behaves the same way for shortcut ownership, focus, navigation, highlighting, and dismissal.

## Requirements

### REQ-IVF-001: Eligible Surface Ownership

WHEN the user presses `Cmd+F` or `Ctrl+F`
THE SYSTEM SHALL open Phoenix in-viewer find for the topmost eligible text surface
AND SHALL prevent the browser's native find UI for that interaction

IF the topmost visible scope is not an eligible text surface
THE SYSTEM SHALL leave the shortcut to its normal browser or control behavior

Eligible text surfaces SHALL include:
- file viewers for text-like payloads
- diff viewers for rendered diff text
- task approval readers
- conversation transcript views when Phoenix renders the searchable text surface directly

Ineligible surfaces SHALL include:
- image-only viewers
- binary or opaque viewers with no text projection
- HTML preview if the preview content is rendered in a separate browsing context without a supported same-document text adapter

**Rationale:** Browser find only sees mounted DOM. Phoenix must own find on virtualized or layered text surfaces to avoid promising matches it cannot actually reach.

---

### REQ-IVF-002: Search the Logical Content, Not Just Mounted DOM

WHEN in-viewer find is open
THE SYSTEM SHALL search the complete logical content of the active eligible surface
AND SHALL include content that is temporarily unmounted because of virtualization, pagination, or collapse/expand rendering strategies, so long as Phoenix's typed render model represents that content

IF a rendered surface omits text from Phoenix's typed render model by design
THE SYSTEM SHALL exclude that omitted text from both search results and result counts
AND SHALL NOT claim matches that Phoenix cannot navigate to

**Rationale:** A search result is only useful if Phoenix can navigate to the underlying content. Logical-content search removes the virtualization blind spot without inventing matches from hidden or out-of-scope data.

---

### REQ-IVF-003: Query Semantics and Result Ordering

THE SYSTEM SHALL treat in-viewer find as a case-insensitive literal substring search
AND an empty query SHALL produce no matches

WHEN the query changes
THE SYSTEM SHALL recompute the result set against the active surface's current logical content
AND SHALL reset the active match to the first result in document order

Match ordering SHALL follow the user-visible reading order of that surface
AND multiple matches in the same line, block, or row SHALL preserve left-to-right order within that container

**Rationale:** Predictable literal matching and visual ordering let users build muscle memory across file, diff, transcript, and approval surfaces.

---

### REQ-IVF-004: Match Counts and Navigation

WHEN a non-empty query has matches
THE SYSTEM SHALL show the active ordinal and total match count
AND SHALL allow `Enter` and the Next control to advance to the next match with wraparound
AND SHALL allow `Shift+Enter` and the Previous control to move to the previous match with wraparound

WHEN a non-empty query has no matches
THE SYSTEM SHALL show a no-results state
AND SHALL disable match-navigation controls that require an existing match

**Rationale:** The user needs both confidence that search found the intended text and a low-friction way to step through every occurrence.

---

### REQ-IVF-005: Navigation to Off-Screen Matches

WHEN the active match is outside the mounted viewport
THE SYSTEM SHALL scroll or mount the owning row, line, or block before revealing the match
AND SHALL complete navigation without requiring the user to scroll manually first

IF the owning surface virtualizes or lazily mounts content
THE SYSTEM SHALL use a stable typed navigation target rather than DOM-node identity to reach the active match

**Rationale:** Off-screen navigation is the core reason Phoenix needs its own find. A result that only works after manual scrolling is a broken contract.

---

### REQ-IVF-006: Visible Match Indication

WHEN in-viewer find has an active match
THE SYSTEM SHALL visibly distinguish the active rendered occurrence from non-active rendered matches
AND rendered non-active matches SHALL remain visibly highlighted where the renderer can support it without lying about hidden content

IF a renderer cannot express exact substring highlighting for an otherwise valid match
THE SYSTEM SHALL still reveal the owning container and provide a clear active-location indication
AND the surface's executive documentation SHALL describe that limitation

**Rationale:** Navigation without a visible target forces the user to hunt again after every jump. Distinguishing the active occurrence from the rest preserves orientation.

---

### REQ-IVF-007: Focus Lifecycle

WHEN in-viewer find opens
THE SYSTEM SHALL focus and select the query input immediately

WHEN the user presses `Cmd/Ctrl+F` again while find is already open for the same surface
THE SYSTEM SHALL keep the existing find bar open
AND SHALL refocus and reselect the current query text

WHEN in-viewer find closes
THE SYSTEM SHALL restore focus to the element that held focus within the same surface before find opened

**Dependencies:** `specs/keyboard-interaction/` REQ-KB-004 and REQ-KB-004A define the general focus and repeated-shortcut expectations this surface-specific behavior instantiates.

**Rationale:** Find is a keyboard-first affordance. Opening it, revisiting it, and leaving it must preserve flow.

---

### REQ-IVF-008: Escape Closes Find Before the Enclosing Surface

WHEN in-viewer find is open and the user presses Escape
THE SYSTEM SHALL close the find bar
AND SHALL NOT close the enclosing viewer, transcript, or task approval surface as part of that same keypress

IF another higher-priority sub-context sits above the viewer surface
THE SYSTEM SHALL allow that topmost sub-context to own Escape first according to the keyboard interaction model

**Dependencies:** `specs/keyboard-interaction/` REQ-KB-005 defines the general topmost-sub-context Escape hierarchy.

**Rationale:** Users expect Escape to close the nearest thing first. Closing the entire viewer when they intended to dismiss only find breaks trust.

---

### REQ-IVF-009: Scope-Local State and Overlay Isolation

THE SYSTEM SHALL keep in-viewer find state local to the mounted eligible surface that owns it
AND SHALL clear that state when the owning surface unmounts or when Phoenix switches to a different underlying content instance

WHEN a higher-priority overlay obscures an eligible surface
THE obscured surface SHALL NOT react to `Cmd/Ctrl+F`, Enter, Shift+Enter, or Escape intended for the topmost scope

**Dependencies:** `specs/keyboard-interaction/` REQ-KB-002A and REQ-KB-008 define the topmost-scope routing guarantees this feature relies on.

**Rationale:** Search state must not leak between unrelated viewers or from a hidden surface into a topmost overlay.

---

### REQ-IVF-010: Editable Controls Keep Their Native Editing Behavior

WHEN focus is inside an editable control that is not the in-viewer find input
THE SYSTEM SHALL leave native text-editing behavior intact unless the topmost eligible surface explicitly documents a stronger shortcut override

WHEN focus is inside the in-viewer find input
THE SYSTEM SHALL keep standard text entry, selection, and cursor movement behavior intact while still supporting find-local navigation keys it owns

**Rationale:** A user typing in a notes field, annotation dialog, or other editor should not have that interaction stolen by a hidden or lower-priority find handler.

---

### REQ-IVF-011: Result Reconciliation as Content Changes

WHEN the active surface's logical content changes while find is open
THE SYSTEM SHALL recompute matches against the new logical content
AND SHALL preserve the current active match when Phoenix can still identify the same logical occurrence in the updated content

IF the prior active occurrence no longer exists
THE SYSTEM SHALL choose the nearest valid active result in document order
OR show no-results if none remain

**Rationale:** Streaming transcript text and live viewer updates should not make find jump unpredictably or keep pointing at a vanished occurrence.

---

### REQ-IVF-012: Accessibility and Keyboard-Only Operation

THE SYSTEM SHALL expose the find bar as an accessible search affordance with a labelled query input, result status text, and keyboard-reachable Previous, Next, and Close controls

A keyboard-only user SHALL be able to:
- open find from an eligible surface
- type a query
- move through matches
- close find
- resume interaction with the enclosing surface

**Rationale:** In-viewer find exists to support keyboard-driven reading and review. If it requires mouse repair steps, it has failed its core job.
