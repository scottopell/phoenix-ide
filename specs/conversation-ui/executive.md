# Conversation UI - Executive Summary

## Scope and Boundary

This spec governs the **conversation experience** — list, chat view, composition, agent-activity display, and the responsive layout that holds them. It does NOT govern the broader UI surface. Phoenix's UI is split across multiple feature specs; readers should not expect this document to describe everything they see in the app.

**In scope here:**
- Conversation list and selection
- Chat view: message history, streaming, conversation navigation, state bar
- Conversation view density (full / compact)
- Conversation navigation strip (whole-conversation chapters)
- Message composition (text, drafts, send/cancel)
- Connection + reconnection visibility
- Sidebar (desktop) / bottom sheet (mobile) for new-conversation entry

**Owned by other specs (cross-references, not duplication):**
- `specs/file-explorer/` — left/right file-tree column on wide desktop
- `specs/command-palette/` — Cmd-K command palette
- `specs/viewer_slot/` — when prose / diff / browser viewer replaces or sits beside the chat column
- `specs/voice-input/` — voice recording + transcription
- `specs/ask-user-question/` — `QuestionPanel`, `TaskApprovalReader` rendering
- `specs/browser-tool/` — `BrowserViewPanel` rendering
- `specs/notifications/` — toast + browser notification policy
- `specs/inline-references/` — `@file`, `/skill` autocomplete inside InputArea
- `specs/keyboard-interaction/` — global keyboard shortcuts

Surfaces that do not yet have their own spec (TerminalPanel, TasksPanel, SkillsPanel, ConversationSettings, RenameDialog, MessageContextMenu, ImageAttachments) are tracked as a follow-up; this document does NOT claim coverage of them just because they live in `ui/src/components/`.

## Requirements Summary

Within the scope above: a responsive interface for conversations with the AI agent across mobile and desktop. Users can view and manage conversations, compose messages with draft persistence, and monitor agent activity in real-time. The interface handles unreliable network connectivity gracefully with optimistic UI, automatic reconnection, and offline message queueing. Desktop users get a persistent sidebar layout with conversation list alongside the active chat. New conversation creation goes through a single dedicated route (`/new`) regardless of entry point — sidebar "+ New" navigates there, the responsive layout inside the page adapts to viewport (full-page form on desktop, bottom-sheet styling on mobile). The form supports "Send in Background" for spawning work without navigating away from a previous conversation.

## Technical Summary

React 18 SPA with React Router, Vite build tooling, and CSS variables for theming. Conversation state managed via a single `ConversationAtom` + `useReducer` in a router-level React context — all SSE events flow through one pure reducer, eliminating split-brain between independent `useState` atoms. `ConversationState` is a TypeScript discriminated union with `satisfies never` on every switch — new backend state variants are compile errors. `agentWorking` is a derived selector, not maintained state. `lastSequenceId` lives in the atom and survives navigation, replacing unbounded `seenIdsRef` with O(1) idempotency. Token streaming accumulates in `streamingBuffer` on the atom; the `sse_message` action clears it atomically in one render (REQ-CONV-019 no-flicker). `appMachine.ts` (previously dead code) is wired as the live implementation via `useAppMachine.ts`. A shared `PillStrip` primitive backs the conversation navigation strip and compact-mode tool strip. Conversation view density (`full`/`compact`) is a presentational preference layered over the canonical `buildRenderUnits` output; compact prose collapse is bounded by `SIGNIFICANCE_THRESHOLD` and gated by whether the preview hides content. `buildConversationChapters` uses the same significance threshold to derive the navigation strip's chapters and jumps the virtualized list by render-unit index (`scrollToUnitIndex`) rather than by DOM query.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-CONV-001:** Conversation List | ✅ Complete | List with slug, cwd, timestamps |
| **REQ-CONV-002:** Chat View | ✅ Complete | Messages, markdown, tool grouping |
| **REQ-CONV-003:** Message Composition | ✅ Complete | Auto-resize and draft persistence; Enter/Shift+Enter and IME-safe submission; non-overlapping responsive controls; concurrent Stop and steering Queue actions while busy |
| **REQ-CONV-004:** Message Delivery States | ✅ Complete | Sending/sent/failed with retry |
| **REQ-CONV-005:** Connection Status | ✅ Complete | Reconnection with backoff |
| **REQ-CONV-006:** Reconnection Data Integrity | ✅ Complete | Sequence-based deduplication |
| **REQ-CONV-007:** Agent Activity Indicators | ✅ Complete | Tasks 579, 581. Discriminated union, exhaustive switch, sequenceId dedup |
| **REQ-CONV-008:** Cancellation | ✅ Complete | Cancel button during agent work |
| **REQ-CONV-009:** New Conversation | ⚠️ Deprecated | Replaced by REQ-CONV-015, 017, 018 |
| **REQ-CONV-010:** Responsive Layout | ✅ Complete | Viewport-specific layouts |
| **REQ-CONV-011:** Local Storage Schema | ✅ Complete | Namespaced keys |
| **REQ-CONV-012:** Conversation State Indicators | ✅ Complete | Part of task 561 |
| **REQ-CONV-013:** Per-Conversation Scroll Position | ⚠️ Deprecated | Removed wholesale; returning lands pinned to bottom |
| **REQ-CONV-014:** Desktop Message Readability | ✅ Complete | Part of task 561 |
| **REQ-CONV-015:** Mobile New Conversation Bottom Sheet | ✅ Complete | Part of task 561 |
| **REQ-CONV-016:** Desktop Sidebar Layout | ✅ Complete | Task 563 |
| **REQ-CONV-017:** Desktop New Conversation - Full Page Mode | ✅ Complete | Task 563 |
| **REQ-CONV-018:** Desktop New Conversation - Inline Sidebar Mode | ✅ Complete | Task 563 |
| **REQ-CONV-019:** Streaming Text Display | ✅ Complete | Task 582. `StreamingMessage` component, atomic swap in reducer |
| **REQ-CONV-020:** Navigation Persistence | ✅ Complete | Task 581. Router-level `ConversationProvider`, `lastSequenceId` in atom |
| **REQ-CONV-021:** Error Resume Affordance | ✅ Complete | User-resumable typed errors surface a resume affordance |
| **REQ-CONV-022:** Conversation View Density | ✅ Complete | `full`/`compact` via `DensityProvider`; compact collapses per-turn tools + text previews that hide content, no data loss |
| **REQ-CONV-023:** Conversation Navigation | ✅ Complete | `ConversationNav` chapter strip; virtuoso `scrollToUnitIndex` jump + scroll-spy; owns the top slot |

**Progress:** 21 of 21 active requirements complete (2 deprecated)
