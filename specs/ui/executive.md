# Web UI - Executive Summary

## Requirements Summary

The Phoenix web UI provides a responsive interface for conversations with the AI agent across mobile and desktop. Users can view and manage conversations, compose messages with draft persistence, and monitor agent activity in real-time. The interface handles unreliable network connectivity gracefully with optimistic UI, automatic reconnection, and offline message queueing. Desktop users get a persistent sidebar layout with conversation list alongside the active chat. New conversation creation adapts to context: full-page form on desktop root, inline sidebar form when viewing a conversation, bottom sheet on mobile. All modes support "Send in Background" for spawning work without navigating away.

## Technical Summary

React 18 SPA with React Router, Vite build tooling, and CSS variables for theming. SSE streaming for real-time updates with sequence-based reconnection for data integrity. localStorage persistence for drafts, message queues, scroll positions, and reconnection state. Message delivery uses three-state model (sending/sent/failed) with automatic retry. Connection manager implements exponential backoff reconnection. State indicators derive from backend `ConvState` exposed via conversation list API. Bottom sheet uses CSS transforms with touch gesture handling for dismiss.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-UI-001:** Conversation List | ✅ Complete | List with slug, cwd, timestamps |
| **REQ-UI-002:** Chat View | ✅ Complete | Messages, markdown, tool grouping |
| **REQ-UI-003:** Message Composition | ✅ Complete | Auto-resize, draft persistence |
| **REQ-UI-004:** Message Delivery States | ✅ Complete | Sending/sent/failed with retry |
| **REQ-UI-005:** Connection Status | ✅ Complete | Reconnection with backoff |
| **REQ-UI-006:** Reconnection Data Integrity | ✅ Complete | Sequence-based deduplication |
| **REQ-UI-007:** Agent Activity Indicators | ✅ Complete | Breadcrumb trail, state display |
| **REQ-UI-008:** Cancellation | ✅ Complete | Cancel button during agent work |
| **REQ-UI-009:** New Conversation | ⚠️ Deprecated | Replaced by REQ-UI-015, 017, 018 |
| **REQ-UI-010:** Responsive Layout | ✅ Complete | Viewport-specific layouts |
| **REQ-UI-011:** Local Storage Schema | ✅ Complete | Namespaced keys |
| **REQ-UI-012:** Conversation State Indicators | ✅ Complete | Part of task 561 |
| **REQ-UI-013:** Per-Conversation Scroll Position | ✅ Complete | Part of task 561 |
| **REQ-UI-014:** Desktop Message Readability | ✅ Complete | Part of task 561 |
| **REQ-UI-015:** Mobile New Conversation Bottom Sheet | ✅ Complete | Part of task 561 |
| **REQ-UI-016:** Desktop Sidebar Layout | ✅ Complete | Task 563 |
| **REQ-UI-017:** Desktop New Conversation - Full Page Mode | ✅ Complete | Task 563 |
| **REQ-UI-018:** Desktop New Conversation - Inline Sidebar Mode | ✅ Complete | Task 563 |
| **REQ-UI-019:** Streaming Text Display | ❌ Not Started | Progressive text display during LLM generation |

**Progress:** 17 of 18 active requirements complete (1 deprecated)
