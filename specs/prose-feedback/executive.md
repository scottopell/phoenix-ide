# Prose Feedback UI - Executive Summary

## Requirements Summary

The Prose Feedback feature enables users to review text files and provide structured, line-level feedback to the AI agent. Users open a file from the file browser, which displays in a full-screen reading view with appropriate formatting (rendered markdown, syntax-highlighted code, or plain text). Long-pressing on any line opens an annotation dialog where users type a note about that specific content. Notes accumulate in a session-local collection, visible via a badge and expandable notes panel. Users can review, delete, or jump to annotated lines before sending. When ready, tapping Send formats all notes into a structured message showing line numbers and content previews, then injects this into the message input for the user to review and send to the conversation. Closing the reader with unsaved notes prompts for confirmation to prevent accidental loss.

## Technical Summary

This is a frontend-only feature requiring no backend changes beyond an existing file read endpoint. The ProseReader component renders as a fixed full-screen overlay, using `react-markdown` with GFM support for documentation files and `react-syntax-highlighter` for code. Long-press detection uses touch event handlers with a 500ms timer that cancels on movement to avoid conflicting with scroll. Notes are stored in component state as an array of objects containing line number, content preview, and user note. The notes panel uses a bottom-drawer pattern with slide-up animation. Formatted output uses markdown blockquote syntax for line references. Integration with the message input uses a callback pattern where the parent component receives the formatted string and appends it to the draft state. All styles are namespaced with `prose-reader-*` prefix. The feature requires adding npm dependencies for markdown and syntax highlighting.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-PF-001:** Open File for Review | ❌ Not Started | - |
| **REQ-PF-002:** Select Content for Annotation | ❌ Not Started | - |
| **REQ-PF-003:** Add Annotation Note | ❌ Not Started | - |
| **REQ-PF-004:** View and Manage Notes | ❌ Not Started | - |
| **REQ-PF-005:** Send Notes to Conversation | ❌ Not Started | - |
| **REQ-PF-006:** Unsaved Notes Warning | ❌ Not Started | - |
| **REQ-PF-007:** Note Persistence Within Session | ❌ Not Started | - |
| **REQ-PF-008:** Responsive Layout | ❌ Not Started | - |
| **REQ-PF-009:** Loading and Error States | ❌ Not Started | - |

**Progress:** 0 of 9 complete

## Prerequisites

- File read API endpoint must exist or be created
- npm dependencies: `react-markdown`, `remark-gfm`, `react-syntax-highlighter`
- File browser component to trigger opening files
