# File Explorer Panel

## User Story

As a desktop user, I need a persistent file tree panel alongside my conversations so that I can browse project files, quickly open them for review, and maintain context of the codebase while chatting with the agent.

## Requirements

### REQ-FE-001: Three-Column Desktop Layout

WHEN viewport is desktop-sized (> 1024px)
THE SYSTEM SHALL display a three-column layout:
  - Left: Conversation sidebar (per REQ-UI-016)
  - Center: File explorer panel (hosts FileTree component)
  - Right: Main content area (conversation or prose reader)

WHEN viewport is below desktop threshold
THE SYSTEM SHALL hide the file explorer panel
AND display FileTree in a modal overlay when file browsing is triggered

**Rationale:** Desktop users have sufficient screen width for persistent file navigation alongside conversations. FileTree is a single responsive component hosted in different containers based on viewport.

---

### REQ-FE-002: File Tree Display

WHEN file explorer panel is visible
THE SYSTEM SHALL display a tree view of the current conversation's working directory
AND show folders and files with appropriate icons (per REQ-PF-004)
AND sort directories first, then files, alphabetically
AND allow expanding/collapsing directories inline

WHEN a directory is expanded
THE SYSTEM SHALL fetch and display its contents
AND persist expansion state for the current conversation

WHEN conversation changes
THE SYSTEM SHALL update the tree root to the new conversation's cwd
AND restore that conversation's previously saved expansion state (if any)

WHEN user expands or collapses a directory
THE SYSTEM SHALL persist expansion state per conversation
AND retain expansion state when switching between conversations

**Rationale:** The file tree reflects the conversation's project context. Expansion state helps users maintain their place when reviewing multiple files.

---

### REQ-FE-003: File Selection

WHEN user clicks a text file in the file tree
THE SYSTEM SHALL open the file in the prose reader
AND display the prose reader in the main content area (replacing conversation view)
AND keep conversation sidebar and file explorer panel visible

WHEN user clicks a non-text file
THE SYSTEM SHALL show the file as disabled (not clickable)
AND indicate "Non-text file" via visual treatment

WHEN prose reader is open and user clicks the conversation in sidebar
THE SYSTEM SHALL close the prose reader
AND return to the conversation view

**Rationale:** Click-to-view is the simplest interaction. Keeping the sidebar and file tree visible maintains navigation context.

---

### REQ-FE-004: Panel Collapse - Expanded State

WHEN file explorer panel is expanded
THE SYSTEM SHALL display the full file tree
AND show a collapse toggle button
AND use a fixed width (approximately 240-280px)

WHEN user clicks the collapse toggle
THE SYSTEM SHALL collapse the panel to its minimal state
AND persist collapse preference to localStorage

**Rationale:** Users may want to maximize main content area temporarily. Persistence respects user preference across sessions.

---

### REQ-FE-005: Panel Collapse - Collapsed State

WHEN file explorer panel is collapsed
THE SYSTEM SHALL display a narrow strip (approximately 48px)
AND show icons for recently opened files (last 3-5 files)
AND show an expand toggle button

WHEN user clicks a recent file icon in collapsed state
THE SYSTEM SHALL open that file in the prose reader
AND NOT expand the panel

WHEN user clicks the expand toggle
THE SYSTEM SHALL expand the panel to full tree view

WHEN user hovers over the collapsed panel
THE SYSTEM MAY show a temporary expanded preview (optional enhancement)

**Rationale:** Recent files provide quick access without requiring full panel expansion. This mirrors the conversation sidebar's collapsed state showing recent conversation indicators.

---

### REQ-FE-006: Recent Files Tracking

WHEN user opens a file in the prose reader
THE SYSTEM SHALL add that file to the recent files list
AND move it to the top if already present
AND limit the list to 5 most recent files

WHEN tracking recent files
THE SYSTEM SHALL store per-conversation
AND persist to localStorage
AND clear when conversation is deleted

**Rationale:** Recent files enable quick re-access to files being actively reviewed, especially useful in collapsed panel state.

---

### REQ-FE-007: Accordion Panel Behavior

WHEN both conversation sidebar and file explorer panel are present
THE SYSTEM SHALL allow each to be independently collapsed/expanded
AND persist each panel's state separately
AND ensure at least the main content area remains visible at all times

WHEN calculating panel widths
THE SYSTEM SHALL use fixed widths for sidebar and file explorer
AND give remaining width to main content area
AND enforce minimum main content width (approximately 400px)

**Rationale:** Independent panel control lets users optimize their workspace. Fixed panel widths provide predictable layout.

---

### REQ-FE-008: Prose Reader Integration

WHEN prose reader opens from file explorer click
THE SYSTEM SHALL render ProseReader component in the main content area
AND pass the selected file path and conversation's rootDir
AND provide a close/back mechanism to return to conversation

WHEN prose reader is displaying a file
THE SYSTEM SHALL highlight the corresponding file in the file tree
AND support all existing prose reader functionality (annotation, notes, send)

WHEN user sends notes from prose reader
THE SYSTEM SHALL inject notes into conversation input (per REQ-PF-009)
AND close the prose reader
AND return to conversation view

**Rationale:** File explorer is a navigation enhancement; prose reader functionality remains unchanged.

---

### REQ-FE-009: Visual Feedback

WHEN a file is currently open in prose reader
THE SYSTEM SHALL highlight it in the file tree with distinct styling

WHEN a file is in the recent files list
THE SYSTEM SHALL show a subtle indicator in the tree (optional)

WHEN loading directory contents
THE SYSTEM SHALL show inline loading indicator on the expanding folder

**Rationale:** Visual feedback keeps users oriented in the file tree.

---

### REQ-FE-010: Mobile File Browser Overlay

WHEN user triggers file browsing on mobile/tablet viewport
THE SYSTEM SHALL display a modal overlay hosting the FileTree component
AND show a header with current path and close button
AND allow dismissal via close button or backdrop tap

WHEN file is selected in the mobile overlay
THE SYSTEM SHALL open the prose reader (full-screen overlay)
AND close the file browser overlay

**Rationale:** Mobile uses modal overlay for focused file browsing. The same FileTree component renders in both desktop panel and mobile overlay contexts.
