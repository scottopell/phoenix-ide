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
| **REQ-AUQ-001:** Structured Question Presentation | ❌ Not Started | - |
| **REQ-AUQ-002:** Rich Option Previews | ❌ Not Started | - |
| **REQ-AUQ-003:** Flexible Response Collection | ❌ Not Started | - |
| **REQ-AUQ-004:** Response Delivery to Agent | ❌ Not Started | - |
| **REQ-AUQ-005:** Prevent Ambiguous Question Responses | ❌ Not Started | - |
| **REQ-AUQ-006:** Parent Conversation Availability | ❌ Not Started | - |
| **REQ-AUQ-007:** Real-Time Waiting Feedback | ❌ Not Started | - |
| **REQ-AUQ-008:** Low-Overhead Tool Availability | ❌ Not Started | - |

**Progress:** 0 of 8 complete
