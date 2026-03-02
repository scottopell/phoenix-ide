# Prose Feedback UI - Executive Summary

## Requirements Summary

The Prose Feedback feature enables users to browse project files and provide structured, line-level feedback to the AI agent. On mobile/tablet, users open a file browser overlay from the conversation interface, navigate directories, and select text files to review. On desktop, the File Explorer Panel (`specs/file-explorer/`) provides persistent file browsing. Selected files display in a reading view with appropriate formatting (rendered markdown, syntax-highlighted code, or plain text). Long-pressing on any line opens an annotation dialog where users type a note about that specific content. Notes accumulate in a session-local collection, visible via a badge and expandable notes panel. Users can review, delete, or jump to annotated lines before sending. When ready, tapping Send formats all notes into a structured message showing the absolute file path, line numbers, and complete raw line content (for greppability), then injects this into the message input. Additionally, patch tool output displays a summary of modified files with change counts, allowing users to click any file to review it with all modifications highlighted. Closing the reader with unsaved notes prompts for confirmation.

## Technical Summary

This feature consists of two main components: FileBrowser and ProseReader. On mobile, FileBrowser renders as a modal overlay; on desktop, it's superseded by the File Explorer Panel (`specs/file-explorer/`). The FileBrowser fetches directory listings from a backend API endpoint and manages navigation state. It detects file types by extension, sorts items (directories first, then alphabetical), and displays metadata like size and modification time. File selection triggers the ProseReader. On mobile, ProseReader renders as a full-screen overlay; on desktop, it renders in the main content area. The ProseReader uses `react-markdown` with GFM support for documentation files and `react-syntax-highlighter` for code. Long-press detection uses touch event handlers with a 500ms timer that cancels on movement. Notes are stored in component state as an array of objects containing line number, full raw content, and user note. The notes panel uses a bottom-drawer pattern with slide-up animation. Formatted output includes the absolute file path and uses markdown quote blocks for raw line content. The ProseReader also supports a system-triggered task approval mode: when the backend emits a `TaskApprovalRequested` SSE event, the UI automatically opens the ProseReader on the specified task file and renders an approval toolbar with Approve, Discard, and Send Feedback actions. Feedback routes back to the agent; approval or discard resolves the `AwaitingTaskApproval` state.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-PF-001:** Browse Project Files | ✅ Complete | Mobile overlay; desktop uses File Explorer Panel |
| **REQ-PF-002:** File Listing Display | ✅ Complete | Size, time, icons, sorting, disabled non-text |
| **REQ-PF-003:** File Browser Navigation | ✅ Complete | Persistent expansion state per conversation |
| **REQ-PF-004:** File Type Detection | ✅ Complete | Backend extension-based detection |
| **REQ-PF-005:** Open File for Review | ✅ Complete | Mobile overlay; desktop in main content |
| **REQ-PF-006:** Select Content for Annotation | ✅ Complete | Long-press with 10px threshold |
| **REQ-PF-007:** Add Annotation Note | ✅ Complete | Dialog with keyboard shortcuts |
| **REQ-PF-008:** View and Manage Notes | ✅ Complete | Badge, notes panel, jump-to-line |
| **REQ-PF-009:** Send Notes to Conversation | ✅ Complete | Formatted with absolute path + raw content |
| **REQ-PF-010:** Unsaved Notes Warning | ✅ Complete | Confirmation dialog |
| **REQ-PF-011:** Note Persistence Within Session | ✅ Complete | Notes cleared on close |
| **REQ-PF-012:** Responsive Layout | ✅ Complete | Mobile overlay; desktop per `specs/file-explorer/` |
| **REQ-PF-013:** Loading and Error States | ✅ Complete | Loading indicators, error messages |
| **REQ-PF-014:** Open File from Patch Tool Output | ✅ Complete | PatchFileSummary with diff parsing |
| **REQ-PF-015:** System-Triggered Prose Reader for Task Approval | ❌ Not Started | Auto-opens on AwaitingTaskApproval state entry |
| **REQ-PF-016:** Approve, Discard, and Feedback Actions for Task Approval | ❌ Not Started | Three-action toolbar; iterative feedback loop |

**Progress:** 14 of 16 complete

## Prerequisites

- ✅ Backend API endpoints: `/api/files/list` and `/api/files/read` implemented
- ✅ npm dependencies: `react-markdown`, `remark-gfm`, `react-syntax-highlighter`, `lucide-react`
- ✅ File browser button integrated in InputArea component

## Related Specs

- `specs/file-explorer/` — Desktop File Explorer Panel (supersedes overlay file browsing on desktop)
