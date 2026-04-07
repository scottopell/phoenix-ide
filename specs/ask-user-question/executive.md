# Ask User Question Tool - Executive Summary

## Requirements Summary

The ask_user_question tool enables LLM agents to pause execution and ask the
user 1-4 structured multiple-choice questions when multiple valid approaches
exist. Users select from predefined options or provide free-text answers. For
questions comparing concrete artifacts (code, config), options can include rich
previews displayed side-by-side. Users can add notes to their selections for
additional context. Responses are delivered back to the agent as a formatted
tool result. The tool is excluded from sub-agents, which operate autonomously.

## Technical Summary

Follows the `AwaitingTaskApproval` state machine pattern: executor intercepts
the tool call, emits `AskUserQuestionPending`, state machine transitions to
`AwaitingUserResponse`, SSE notifies the UI, user responds via
`POST /conversations/{id}/respond`, `UserQuestionResponse` event resumes
execution. Input validation enforces question/option count constraints and
uniqueness. The tool is registered with `defer_loading: true` for tool search
on supporting models. Tool result format is a human-readable string including
selected labels, preview content, and user notes.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-AUQ-001:** Structured Question Presentation | ✅ Complete | `src/tools/ask_user_question.rs`, `AwaitingUserResponse` state, `QuestionPanel.tsx` |
| **REQ-AUQ-002:** Rich Option Previews | ✅ Complete | Side-by-side `question-preview-layout` in `QuestionPanel.tsx`/`.css` |
| **REQ-AUQ-003:** Flexible Response Collection | ✅ Complete | `OTHER_SENTINEL`, `otherTexts`, `multiSelections`, notes/annotations |
| **REQ-AUQ-004:** Response Delivery to Agent | ✅ Complete | `POST /api/conversations/:id/respond`, `UserQuestionResponse` event |
| **REQ-AUQ-005:** Prevent Ambiguous Question Responses | ✅ Complete | Schema constraints in tool; `AwaitingUserResponse` blocks `UserMessage` in `transition.rs` |
| **REQ-AUQ-006:** Parent Conversation Availability | ✅ Complete | Excluded from sub-agent `ToolRegistry` in `src/tools.rs` |
| **REQ-AUQ-007:** Real-Time Waiting Feedback | ✅ Complete | `awaiting_user_response` in `ConversationState` SSE union in `ui/src/api.ts` |
| **REQ-AUQ-008:** Low-Overhead Tool Availability | ✅ Complete | `defer_loading() -> bool { true }` in `ask_user_question.rs` |

**Progress:** 8 of 8 complete
