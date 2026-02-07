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
| **REQ-PF-001:** Browse Project Files | ✅ Complete | FileBrowser component with API integration |
| **REQ-PF-002:** File Listing Display | ✅ Complete | Size, time, icons, sorting, disabled non-text |
| **REQ-PF-003:** File Browser Navigation | ✅ Complete | Persistent expansion state per conversation |
| **REQ-PF-004:** File Type Detection | ✅ Complete | Backend extension-based detection |
| **REQ-PF-005:** Open File for Review | ✅ Complete | ProseReader with markdown + syntax highlighting |
| **REQ-PF-006:** Select Content for Annotation | ✅ Complete | Long-press with 10px threshold |
| **REQ-PF-007:** Add Annotation Note | ✅ Complete | Dialog with keyboard shortcuts |
| **REQ-PF-008:** View and Manage Notes | ✅ Complete | Badge, notes panel, jump-to-line |
| **REQ-PF-009:** Send Notes to Conversation | ✅ Complete | Formatted with absolute path + raw content |
| **REQ-PF-010:** Unsaved Notes Warning | ✅ Complete | Confirmation dialog |
| **REQ-PF-011:** Note Persistence Within Session | ✅ Complete | Notes cleared on close |
| **REQ-PF-012:** Responsive Layout | ✅ Complete | Full-screen overlay, 44px touch targets |
| **REQ-PF-013:** Loading and Error States | ✅ Complete | Loading indicators, error messages |
| **REQ-PF-014:** Open File from Patch Tool Output | ✅ Complete | PatchFileSummary with diff parsing |

**Progress:** 14 of 14 complete

## Prerequisites

- ✅ Backend API endpoints: `/api/files/list` and `/api/files/read` implemented
- ✅ npm dependencies: `react-markdown`, `remark-gfm`, `react-syntax-highlighter`, `lucide-react`
- ✅ File browser button integrated in InputArea component
