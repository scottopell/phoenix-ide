# Prose Feedback UI - Executive Summary

## Requirements Summary

## Requirements Summary

The Prose Feedback feature enables users to browse project files and provide structured, line-level feedback to the AI agent. Users open a file browser from the conversation interface, navigate directories, and select text files to review. Selected files display in a full-screen reading view with appropriate formatting (rendered markdown, syntax-highlighted code, or plain text). Long-pressing on any line opens an annotation dialog where users type a note about that specific content. Notes accumulate in a session-local collection, visible via a badge and expandable notes panel. Users can review, delete, or jump to annotated lines before sending. When ready, tapping Send formats all notes into a structured message showing the absolute file path, line numbers, and complete raw line content (for greppability), then injects this into the message input for the user to review and send to the conversation. The file browser shows directories first, then files, both sorted alphabetically with visual type indicators, file sizes, and modification times. Additionally, patch tool output displays a summary of modified files with change counts, allowing users to click any file to review it with all modifications highlighted in a unified view, automatic scrolling to changes, and automatic prefixing for annotations on modified lines. Closing the reader with unsaved notes prompts for confirmation to prevent accidental loss.

## Technical Summary

## Technical Summary

This feature consists of two main components: FileBrowser and ProseReader. The FileBrowser renders as a modal overlay, fetching directory listings from a backend API endpoint and managing navigation state. It detects file types by extension, sorts items (directories first, then alphabetical), and displays metadata like size and modification time. File selection triggers the ProseReader overlay. The ProseReader uses `react-markdown` with GFM support for documentation files and `react-syntax-highlighter` for code. Long-press detection uses touch event handlers with a 500ms timer that cancels on movement. Notes are stored in component state as an array of objects containing line number, full raw content, and user note. The notes panel uses a bottom-drawer pattern with slide-up animation. Formatted output includes the absolute file path and uses markdown code blocks for raw line content to ensure greppability. Integration with the message input uses a callback pattern where the parent component receives the formatted string and appends it to the draft state. All styles are namespaced with component prefixes. The feature requires adding npm dependencies for markdown and syntax highlighting, plus two new backend API endpoints for directory listing and file reading.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-PF-001:** Browse Project Files | ❌ Not Started | - |
| **REQ-PF-002:** File Listing Display | ❌ Not Started | - |
| **REQ-PF-003:** File Browser Navigation | ❌ Not Started | - |
| **REQ-PF-004:** File Type Detection | ❌ Not Started | - |
| **REQ-PF-005:** Open File for Review | ❌ Not Started | - |
| **REQ-PF-006:** Select Content for Annotation | ❌ Not Started | - |
| **REQ-PF-007:** Add Annotation Note | ❌ Not Started | - |
| **REQ-PF-008:** View and Manage Notes | ❌ Not Started | - |
| **REQ-PF-009:** Send Notes to Conversation | ❌ Not Started | - |
| **REQ-PF-010:** Unsaved Notes Warning | ❌ Not Started | - |
| **REQ-PF-011:** Note Persistence Within Session | ❌ Not Started | - |
| **REQ-PF-012:** Responsive Layout | ❌ Not Started | - |
| **REQ-PF-013:** Loading and Error States | ❌ Not Started | - |
| **REQ-PF-014:** Open File from Patch Tool Output | ❌ Not Started | - |

**Progress:** 0 of 14 complete

## Prerequisites

- Backend API endpoints for file listing and reading (see design doc)
- npm dependencies: `react-markdown`, `remark-gfm`, `react-syntax-highlighter`
- Button or menu item in conversation UI to trigger file browser
