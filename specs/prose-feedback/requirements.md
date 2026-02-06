# Prose Feedback UI

## User Story

As a user reviewing text files (markdown documentation, source code, or plain text), I need to browse project files, open them for review, annotate specific lines with notes, and send those notes as a bundled message to the AI agent, so that I can provide structured feedback without manually copying line numbers and content.

## Requirements

### REQ-PF-001: Browse Project Files

WHEN user taps the file browse button in the conversation
THE SYSTEM SHALL display a file browser overlay
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
AND show file sizes for files (human readable: KB, MB)
AND show modification time (relative: "2 hours ago", "3 days ago")

WHEN directory contains more than 100 items
THE SYSTEM SHALL virtualize the list for performance

WHEN directory is empty
THE SYSTEM SHALL show "Empty directory" message

**Rationale:** Consistent ordering and metadata helps users find files efficiently. Virtualization prevents performance issues in large directories.

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

**Rationale:** Clear navigation context prevents users from getting lost in the directory structure.

---

### REQ-PF-004: File Type Detection

WHEN displaying files
THE SYSTEM SHALL detect file types by extension:
  - Markdown: .md, .markdown → 📄 icon
  - Code: .rs, .ts, .tsx, .js, .jsx, .py, .go, .java, .cpp, .c, .h → 🔤 icon
  - Config: .json, .yaml, .yml, .toml, .ini, .env → ⚙️ icon
  - Text: .txt, .log → 📃 icon
  - Other/unknown → 📎 icon
  - Directories → 📁 icon

WHEN file has no extension
THE SYSTEM SHALL treat as text if content appears to be text
OTHERWISE show as unknown type

**Rationale:** Visual file type indicators help users quickly identify relevant files to review.

---

### REQ-PF-005: Open File for Review

WHEN user selects a file from the file browser
THE SYSTEM SHALL display the file content in a full-screen reading overlay
AND show the filename in a header bar
AND provide a back/close button to return to the conversation

WHEN file is markdown format (.md, .markdown)
THE SYSTEM SHALL render the content with formatted headings, lists, code blocks, and emphasis

WHEN file is source code (recognized extensions: .rs, .ts, .tsx, .js, .jsx, .py, .go, .json, .yaml, .yml, .toml, .css, .html)
THE SYSTEM SHALL display with syntax highlighting and line numbers

WHEN file is plain text or unrecognized format
THE SYSTEM SHALL display as monospace text with line numbers

**Rationale:** Users need to read files in a comfortable format before providing feedback. Rendering markdown and highlighting code improves readability.

---

### REQ-PF-006: Select Content for Annotation

WHEN user long-presses (500ms) on a line or paragraph
THE SYSTEM SHALL trigger haptic feedback (if device supports vibration)
AND display an annotation dialog anchored to that content
AND show the line number and a preview of the selected content (first 100 characters)

WHEN user releases before 500ms threshold
THE SYSTEM SHALL NOT open the annotation dialog (treated as normal touch/scroll)

WHEN user moves finger during long-press
THE SYSTEM SHALL cancel the long-press gesture (allow scrolling)

**Rationale:** Long-press is a familiar mobile gesture for contextual actions. The 500ms threshold and movement cancellation prevent accidental triggers while scrolling.

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

WHEN viewport is mobile-sized
THE SYSTEM SHALL use full-screen overlay
AND use bottom drawer for annotation dialog and notes panel
AND ensure touch targets are at least 44px

WHEN viewport is desktop-sized
THE SYSTEM SHALL use full-screen overlay (same as mobile)
AND support mouse hover and click in addition to touch
AND support keyboard navigation (Escape to close dialogs)

**Rationale:** Primary use case is mobile review, but desktop must work for development.

---

### REQ-PF-013: Loading and Error States

WHEN file content is loading
THE SYSTEM SHALL display a loading indicator centered in the content area

WHEN file fails to load
THE SYSTEM SHALL display an error message with the failure reason
AND allow user to close the reader and return to conversation

**Rationale:** Users need feedback when operations are in progress or have failed.
