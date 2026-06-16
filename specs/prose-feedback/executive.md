# Prose Feedback UI - Executive Summary

## Requirements Summary

The Prose Feedback feature enables users to browse project files and provide structured, line-level feedback to the AI agent. On mobile/tablet, users open a file browser overlay from the conversation interface, navigate directories, and select text files to review. On desktop, the File Explorer Panel (`specs/file-explorer/`) provides persistent file browsing. Selected files display in a reading view with appropriate formatting (rendered markdown, syntax-highlighted code, or plain text). Long-pressing on any line opens an annotation dialog where users type a note about that specific content. Notes accumulate in a session-local collection, visible via a badge and expandable notes panel. Users can review, delete, or jump to annotated lines before sending. When ready, tapping Send formats all notes into a structured message showing the absolute file path, line numbers, and complete raw line content (for greppability), then injects this into the message input. Additionally, patch tool output displays a summary of modified files with change counts, allowing users to click any file to review it with all modifications highlighted. Closing the reader with unsaved notes prompts for confirmation.

## Technical Summary

This feature consists of two main parts: FileBrowser and the file-viewer stack. On mobile, FileBrowser renders as a modal overlay; on desktop, it's superseded by the File Explorer Panel (`specs/file-explorer/`). The FileBrowser fetches directory listings from a backend API endpoint and manages navigation state. It detects file types by extension, sorts items (directories first, then alphabetical), and displays metadata like size and modification time.

File selection opens the file viewer. `FileViewer` is the loader: it fetches `/api/files/read`, then classifies the result into a typed `MetaViewerPayload` (markdown / code / text / html / image). Openability and the text/image split are decided once on the server (`FileViewerKind`, the same classification used by the file listing, search, and quick-open) and never re-derived on the client. Render-kind classification is owned by `classifyViewerFile`, which trusts the server's `TextCategory` (the markdown/code/config/plain split from REQ-PF-004) and only layers on the html split — for the source/preview toggle — and the syntax-highlighter language, so the extension→category table is not duplicated between client and server. `MetaViewer` routes a resolved payload to one focused body renderer (`MarkdownViewerBody` via `react-markdown` + GFM, `CodeViewerBody` via `react-syntax-highlighter`, `TextViewerBody`, `HtmlViewerBody`, `ImageViewerBody`) and owns the cross-cutting concerns (scroll restoration, copy, select-all, jump-to-line, the html source/preview toggle). On mobile it renders as a full-screen overlay; on desktop, in the main content area. Long-press detection uses touch event handlers with a 500ms timer that cancels on movement. Review notes live in the conversation-scoped `ReviewNotesContext`; the add/jump/send/clear lifecycle is shared via `useFileReviewNotes` (files) and `useDiffReviewNotes` (diffs). The notes panel uses a bottom-drawer pattern with slide-up animation. Formatted output includes the absolute file path and uses markdown quote blocks for raw line content.

Task approval is a separate component, `TaskApprovalReader`, not part of the MetaViewer file-review stack. When the backend emits a `TaskApprovalRequested` SSE event, the UI opens it on the specified task file and renders an approval toolbar with Approve, Discard, and Send Feedback actions. Feedback routes back to the agent; approval or discard resolves the `AwaitingTaskApproval` state. It is kept separate deliberately: its lifecycle is task approval (a phase-driven overlay that layers on top of the viewer slot, see `specs/viewer_slot/`), not file review; its notes are local and sent as approval feedback rather than added to the conversation review pile; and it is non-dismissible. It reuses the shared annotation long-press idiom but not the `ReviewNotesContext`-bound hooks, since the anchor and submission semantics differ.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-PF-001:** Browse Project Files | ✅ Complete | Mobile overlay; desktop uses File Explorer Panel |
| **REQ-PF-002:** File Listing Display | ✅ Complete | Size, time, icons, sorting, disabled non-viewable (binaries) |
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
