# Prose Feedback UI

## User Story

As a user reviewing text files (markdown documentation, source code, or plain text), I need to annotate specific lines with notes and send those notes as a bundled message to the AI agent, so that I can provide structured feedback without manually copying line numbers and content.

## Requirements

### REQ-PF-001: Open File for Review

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

### REQ-PF-002: Select Content for Annotation

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

### REQ-PF-003: Add Annotation Note

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

### REQ-PF-004: View and Manage Notes

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

### REQ-PF-005: Send Notes to Conversation

WHEN user taps Send (header) or Send All (notes panel)
THE SYSTEM SHALL format all notes as a structured message:
  - Header: "Review notes for `{filename}`:"
  - For each note: "> Line {N}: \"{content preview}\"" followed by the note text
  - Notes separated by blank lines
AND inject the formatted text into the message input field
AND clear all notes
AND close the prose reader overlay

WHEN formatted notes are injected into message input
THE SYSTEM SHALL append to any existing draft text (with blank line separator if draft is non-empty)
AND focus the message input

**Rationale:** Injecting into the input field rather than auto-sending gives users a chance to add context or edit before sending. The structured format helps the AI understand the feedback context.

---

### REQ-PF-006: Unsaved Notes Warning

WHEN user taps back/close with unsaved notes
THE SYSTEM SHALL display a confirmation dialog: "You have N unsaved note(s). Discard them?"
AND provide Cancel and Discard options

WHEN user confirms discard
THE SYSTEM SHALL clear all notes and close the prose reader

WHEN user cancels
THE SYSTEM SHALL return to the prose reader with notes preserved

**Rationale:** Prevents accidental loss of annotation work.

---

### REQ-PF-007: Note Persistence Within Session

WHILE prose reader is open for a file
THE SYSTEM SHALL maintain notes in memory

WHEN prose reader is closed (after send or discard)
THE SYSTEM SHALL NOT persist notes (notes are ephemeral per review session)

WHEN user reopens the same file
THE SYSTEM SHALL start with zero notes (fresh review session)

**Rationale:** Notes are intended for immediate feedback cycles, not long-term storage. Ephemeral notes keep the UX simple.

---

### REQ-PF-008: Responsive Layout

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

### REQ-PF-009: Loading and Error States

WHEN file content is loading
THE SYSTEM SHALL display a loading indicator centered in the content area

WHEN file fails to load
THE SYSTEM SHALL display an error message with the failure reason
AND allow user to close the reader and return to conversation

**Rationale:** Users need feedback when operations are in progress or have failed.
