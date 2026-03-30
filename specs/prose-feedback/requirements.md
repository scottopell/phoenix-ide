# Prose Feedback UI

## User Story

As a user reviewing text files (markdown documentation, source code, or plain text), I need to browse project files, open them for review, annotate specific lines with notes, and send those notes as a bundled message to the AI agent, so that I can provide structured feedback without manually copying line numbers and content.

## Requirements

### REQ-PF-001: Browse Project Files

> **Note:** File browsing UI is defined in `specs/file-explorer/`. This requirement describes the core browsing behavior; REQ-FE-001 and REQ-FE-010 define the desktop panel and mobile overlay hosts respectively.

WHEN user triggers file browsing
THE SYSTEM SHALL display the file tree rooted at the conversation's working directory
AND show the current working directory path
AND list all files and directories in the current directory
AND indicate file types with icons (folder, text, code, markdown)

WHEN user taps a directory
THE SYSTEM SHALL navigate into that directory
AND update the current path display
AND show the new directory contents

WHEN user taps the back/up button
THE SYSTEM SHALL navigate to the parent directory
AND update the listing

WHEN user taps a text file (any extension)
THE SYSTEM SHALL open the prose reader for that file

**Rationale:** Users need to navigate the project structure to find files they want to review. Visual indicators help identify file types quickly.

---

### REQ-PF-002: File Listing Display

WHEN displaying directory contents
THE SYSTEM SHALL show folders first, then files
AND sort each group alphabetically (case-insensitive)
AND display file/folder names with appropriate icons
AND show file sizes for files (human readable: KiB, MiB, GiB)
AND show modification time (relative: "2 hours ago", "3 days ago")
AND show all files including non-text files (images, binaries, etc.)
AND disable non-text files with visual indication (grayed out)
AND show tooltip or subtitle "Non-text file" for disabled items

WHEN directory contains more than 100 items
THE SYSTEM SHALL virtualize the list for performance

WHEN directory is empty
THE SYSTEM SHALL show "Empty directory" message

**Rationale:** Consistent ordering and metadata helps users find files efficiently. Showing all files (even non-reviewable ones) gives complete directory context. Virtualization prevents performance issues in large directories.

---

### REQ-PF-003: File Browser Navigation

WHEN file browser is open
THE SYSTEM SHALL show a header with:
  - Current directory path (truncated if needed with ellipsis)
  - Up/back button (disabled at root)
  - Close button to return to conversation

WHEN path is too long for display
THE SYSTEM SHALL show "..." at the beginning
AND preserve the last 2-3 path segments visible

WHEN user is at the project root
THE SYSTEM SHALL disable the up button

WHEN user expands or collapses a directory
THE SYSTEM SHALL persist this state for the current conversation
AND restore expanded/collapsed states when file browser is reopened
AND maintain separate expansion states per conversation

WHEN conversation ends or user switches conversations
THE SYSTEM SHALL reset all expanded/collapsed states

**Rationale:** Clear navigation context prevents users from getting lost in the directory structure. Persisting folder expansion state within a conversation reduces repetitive navigation actions when reviewing multiple files.

---

### REQ-PF-004: File Type Detection

WHEN displaying files
THE SYSTEM SHALL detect file types by extension:
  - Markdown: .md, .markdown → [icon: document-text]
  - Code: .rs, .ts, .tsx, .js, .jsx, .py, .go, .java, .cpp, .c, .h → [icon: code]
  - Config: .json, .yaml, .yml, .toml, .ini, .env → [icon: settings]
  - Text: .txt, .log → [icon: document]
  - Image: .png, .jpg, .jpeg, .gif, .svg, .webp → [icon: image]
  - Data: .db, .sqlite, .bin, .dat → [icon: database]
  - Other/unknown → [icon: file]
  - Directories → [icon: folder]

WHEN file has no extension
THE SYSTEM SHALL use backend-provided type information
AND backend MAY check shebang for scripts (e.g., `#!/usr/bin/env python`)
AND backend SHALL NOT peek at file contents in large directories

WHEN file is non-text (images, binaries, data files)
THE SYSTEM SHALL show with appropriate icon but in disabled/grayed state
AND NOT allow selection

**Rationale:** Visual file type indicators help users quickly identify relevant files to review. Minimalistic icons maintain a clean, professional interface. Showing but disabling non-text files provides complete directory context without confusion. Backend-side type detection prevents frontend performance issues when browsing large directories.

---

### REQ-PF-005: Open File for Review

> **Note:** File selection triggers are defined in `specs/file-explorer/` (REQ-FE-003). This requirement describes prose reader behavior once a file is selected.

WHEN user selects a text file
THE SYSTEM SHALL display the file content in a full-screen reading overlay
AND show the filename in a header bar
AND provide a back/close button to return to the conversation

WHEN file is markdown format (.md, .markdown)
THE SYSTEM SHALL render the content with formatted headings, lists, code blocks, and emphasis

WHEN file is source code (recognized extensions: .rs, .ts, .tsx, .js, .jsx, .py, .go, .json, .yaml, .yml, .toml, .css, .html)
THE SYSTEM SHALL display with syntax highlighting and line numbers

WHEN file is plain text or unrecognized format
THE SYSTEM SHALL request file content from backend
AND backend SHALL validate text encoding during read
AND display as monospace text with line numbers if valid encoding
AND show error message if backend reports invalid/binary data

**Rationale:** Users need to read files in a comfortable format before providing feedback. Rendering markdown and highlighting code improves readability. Text encoding validation happens during file read (not listing) for performance.

---

### REQ-PF-006: Select Content for Annotation

WHEN user long-presses (500ms) on a line or paragraph
THE SYSTEM SHALL trigger haptic feedback (if device supports vibration)
AND display an annotation dialog anchored to that content
AND show the line number and a preview of the selected content (first 100 characters)

WHEN user releases before 500ms threshold
THE SYSTEM SHALL NOT open the annotation dialog (treated as normal touch/scroll)

WHEN user moves finger during long-press
THE SYSTEM SHALL cancel the long-press gesture immediately
AND allow normal scrolling to continue
AND require movement threshold of only 10 pixels to trigger cancellation

**Rationale:** Long-press is a familiar mobile gesture for contextual actions. The 500ms threshold and aggressive movement cancellation (10px threshold) prevents accidental triggers while scrolling, especially during slow reading scrolls. Users who slowly scroll while reading should never accidentally trigger annotations.

---

### REQ-PF-007: Add Annotation Note

WHEN annotation dialog is open
THE SYSTEM SHALL display a text input for the user's note
AND auto-focus the text input
AND show Cancel and Add Note buttons

WHEN user taps Add Note with non-empty text
THE SYSTEM SHALL create a note associated with the selected line
AND close the annotation dialog
AND clear the note input

WHEN user taps Add Note with empty text
THE SYSTEM SHALL NOT create a note (button should be disabled)

WHEN user taps Cancel or taps outside the dialog
THE SYSTEM SHALL close the annotation dialog without creating a note

WHEN user presses Escape key
THE SYSTEM SHALL close the annotation dialog

WHEN user presses Ctrl+Enter or Cmd+Enter
THE SYSTEM SHALL submit the note (same as tapping Add Note)

**Rationale:** Standard dialog interactions with keyboard shortcuts for power users.

---

### REQ-PF-008: View and Manage Notes

WHEN one or more notes exist
THE SYSTEM SHALL display a badge in the header showing the note count
AND display a Send button in the header

WHEN user taps the note count badge
THE SYSTEM SHALL open a notes panel (bottom drawer on mobile)
AND list all notes showing: line number, content preview (60 chars), and note text
AND provide a delete button for each note
AND provide Clear All and Send All action buttons

WHEN user taps a note's line number in the notes panel
THE SYSTEM SHALL close the notes panel
AND scroll the content view to that line
AND briefly highlight the line (2 second animation)

WHEN user taps delete on a note
THE SYSTEM SHALL remove that note from the list

WHEN user taps Clear All
THE SYSTEM SHALL remove all notes
AND close the notes panel

**Rationale:** Users need to review their notes before sending, correct mistakes, and navigate back to annotated lines.

---

### REQ-PF-009: Send Notes to Conversation

WHEN user taps Send (header) or Send All (notes panel)
THE SYSTEM SHALL format all notes as a structured message:
  - Header: "Review notes for `{absolute_file_path}`:"
  - For each note: "> Line {N}: `{raw_line_content}`" followed by the note text
  - The raw line content SHALL be the exact text from the file (untruncated) to ensure greppability
  - Notes separated by blank lines
AND inject the formatted text into the message input field
AND clear all notes
AND close the prose reader overlay

WHEN formatted notes are injected into message input
THE SYSTEM SHALL append to any existing draft text (with blank line separator if draft is non-empty)
AND focus the message input

**Rationale:** Injecting into the input field rather than auto-sending gives users a chance to add context or edit before sending. The structured format with absolute paths and raw content helps the AI precisely locate and understand the feedback context. Including the complete raw line content ensures the AI can grep for exact matches in the codebase.

---

### REQ-PF-010: Unsaved Notes Warning

WHEN user taps back/close with unsaved notes
THE SYSTEM SHALL display a confirmation dialog: "You have N unsaved note(s). Discard them?"
AND provide Cancel and Discard options

WHEN user confirms discard
THE SYSTEM SHALL clear all notes and close the prose reader

WHEN user cancels
THE SYSTEM SHALL return to the prose reader with notes preserved

**Rationale:** Prevents accidental loss of annotation work.

---

### REQ-PF-011: Note Persistence Within Session

WHILE prose reader is open for a file
THE SYSTEM SHALL maintain notes in memory

WHEN prose reader is closed (after send or discard)
THE SYSTEM SHALL NOT persist notes (notes are ephemeral per review session)

WHEN user reopens the same file
THE SYSTEM SHALL start with zero notes (fresh review session)

**Rationale:** Notes are intended for immediate feedback cycles, not long-term storage. Ephemeral notes keep the UX simple.

---

### REQ-PF-012: Responsive Layout

> **Note:** Desktop layout is now defined in `specs/file-explorer/` (REQ-FE-001). This requirement focuses on mobile/tablet behavior and general accessibility.

WHEN viewport is mobile-sized
THE SYSTEM SHALL use full-screen overlay for file browser and prose reader
AND use bottom drawer for annotation dialog and notes panel
AND ensure touch targets are at least 44px

WHEN viewport is desktop-sized
THE SYSTEM SHALL use File Explorer Panel layout (per `specs/file-explorer/`)
AND render prose reader in main content area (not overlay)
AND support mouse hover and click in addition to touch
AND support keyboard navigation (Escape to close dialogs)

**Rationale:** Mobile uses overlays for focus. Desktop uses persistent panels for context.

---

### REQ-PF-013: Loading and Error States

WHEN file content is loading
THE SYSTEM SHALL display a loading indicator centered in the content area

WHEN file fails to load
THE SYSTEM SHALL display an error message with the failure reason
AND allow user to close the reader and return to conversation

**Rationale:** Users need feedback when operations are in progress or have failed.

---

### REQ-PF-014: Open File from Patch Tool Output

WHEN patch tool generates output with unified diffs (see REQ-PATCH-007)
THE SYSTEM SHALL extract all unique filenames mentioned in the diffs
AND display them as a clickable list at the end of the patch output
AND show count of changes per file (e.g., "file.rs (3 changes)")

WHEN user clicks/taps a filename from the extracted list
THE SYSTEM SHALL open that file in the prose reader
AND parse ALL unified diffs for that file across the entire patch output
AND highlight ALL modified lines from all patches with gentle visual indicators
AND auto-scroll to the first modified line
AND allow normal annotation on any line (not just modified lines)

WHEN displaying patch-triggered file view with multiple changes
THE SYSTEM SHALL show a banner indicating "Viewing {filename}: {N} changes from patch"
AND display the full file content (never collapsed or filtered)
AND merge all modifications into a single set of highlighted lines
AND use consistent highlighting regardless of how many times a line was modified

WHEN user annotates a line that was modified by any patch
THE SYSTEM SHALL prefix the note with "[Changed line]" automatically
AND allow the user to edit or remove this prefix

**Rationale:** When multiple patches modify the same file, users need a unified view showing all changes together. This prevents confusion from viewing the same file multiple times with different highlights. Extracting unique files and showing change counts helps users prioritize which files to review. Integration with REQ-PATCH-007 ensures consistency with patch tool output.

---

### REQ-PF-015: System-Triggered Prose Reader for Task Approval

WHEN a conversation enters AwaitingTaskApproval state
THE SYSTEM SHALL automatically open the prose reader with the plan content
  from the serialized state (title, priority, plan text — NOT from a file on disk)
AND SHALL NOT require user navigation to find or open the file
AND SHALL display an approval toolbar alongside the standard annotation interface

WHEN the prose reader is opened by the system for task approval
THE SYSTEM SHALL clearly indicate that this is a pending task plan awaiting review
AND prevent the user from closing the prose reader without choosing an action
  (Approve, Discard, or Send Feedback)
AND suppress back/Escape-to-close — the user MUST choose an explicit action

WHEN the conversation leaves AwaitingTaskApproval state (for any reason)
THE SYSTEM SHALL automatically close the task approval prose reader

**Rationale:** The task approval flow must be impossible to miss — it is a deliberate
pause for human oversight, not a passive notification. Automatic opening removes the
finding-and-opening friction and makes the required action obvious. Preventing casual
close (without an explicit choice) ensures the user makes a deliberate decision rather
than accidentally dismissing the review.

**Cross-references:** REQ-PROJ-003, REQ-PROJ-004, REQ-BED-028

---

### REQ-PF-016: Approve, Discard, and Feedback Actions for Task Approval

WHEN prose reader is in task approval mode
THE SYSTEM SHALL display three primary actions: Approve, Discard, and Send Feedback

WHEN user taps Approve
THE SYSTEM SHALL resolve the approval with an approved outcome
AND close the task approval prose reader
AND the system SHALL perform all git operations (write task file, commit, branch,
  checkout) as defined in REQ-PROJ-004

WHEN user taps Discard
THE SYSTEM SHALL display a confirmation: "Discard this plan? The conversation will
return to Explore mode."
AND on confirmation, resolve the approval with a rejected outcome
AND close the task approval prose reader
AND NOT perform any git operations (no file was written, nothing to clean up)

WHEN user annotates lines and taps Send Feedback
THE SYSTEM SHALL format the annotations as a structured message (per REQ-PF-009 format)
AND deliver the formatted feedback to the agent as a user message (NOT a tool result —
  the propose_plan tool result was already persisted when entering AwaitingTaskApproval)
AND close the prose reader
AND transition the conversation to Explore/Idle
AND clear annotations after sending

The agent may revise the plan and call `propose_plan` again, which re-enters
AwaitingTaskApproval and opens a fresh prose reader with the updated plan content.
No content reload or keep-open behavior is needed — each feedback cycle is a clean
mount/unmount of the prose reader.

**Rationale:** Three explicit actions make the user's choices unambiguous. Approve and
Discard are terminal decisions; Send Feedback is iterative but each round is a clean
cycle (close, agent revises, reopen). The discard confirmation prevents accidental loss
of task proposals that the agent may have worked to produce.

**Cross-references:** REQ-PROJ-003, REQ-PROJ-004, REQ-BED-028
