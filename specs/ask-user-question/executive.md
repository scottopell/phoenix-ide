# Ask User Question Tool - Executive Summary

## Requirements Summary

The ask_user_question tool enables LLM agents to pause execution and ask the user clarifying questions when multiple valid approaches exist. The agent submits 1-4 structured questions, each with 2-4 predefined options. Users can select from options or provide free-text answers. Responses are delivered back to the agent as a tool result, allowing it to continue with informed decisions. The tool is unavailable to sub-agents, which must operate autonomously.

## Technical Summary

Implemented as a special tool that triggers a state machine transition rather than normal execution. When the executor detects `ask_user_question`, it emits an `AskUserQuestionPending` event that transitions the conversation to `AwaitingUserResponse` state. The SSE state_change event notifies the UI to display questions. User responses arrive via POST to `/conversations/{id}/respond`, generating a `UserQuestionResponse` event that transitions back to tool execution (or LLM request if no tools remain). The tool result contains answers as JSON mapping question text to selected labels.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-AUQ-001:** Question Presentation | ❌ Not Started | - |
| **REQ-AUQ-002:** User Response Collection | ❌ Not Started | - |
| **REQ-AUQ-003:** Response Delivery to Agent | ❌ Not Started | - |
| **REQ-AUQ-004:** Tool Schema | ❌ Not Started | - |
| **REQ-AUQ-005:** Sub-Agent Restriction | ❌ Not Started | - |
| **REQ-AUQ-006:** State Visibility | ❌ Not Started | - |

**Progress:** 0 of 6 complete
