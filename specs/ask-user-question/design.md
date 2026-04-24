# Ask User Question Tool - Technical Design

## Architecture Overview

The ask_user_question tool pauses agent execution and waits for user input.
The execution flow is:

1. The LLM returns a tool_use block for ask_user_question
2. The executor intercepts the tool call before normal dispatch
3. The executor validates the input and emits an `AskUserQuestionPending` event
4. The state machine transitions from `ToolExecuting` to `AwaitingUserResponse`,
   persisting the questions and remaining tool state
5. An SSE state_change event notifies the UI to display the question interface
6. The user selects options and submits via `POST /api/conversations/{id}/respond`
7. The API handler sends a `UserQuestionResponse` event to the state machine
8. The state machine constructs a tool result from the answers and resumes:
   either executing remaining tools or requesting the next LLM turn

The tool struct exists for schema and description purposes. Execution is
intercepted by the executor before reaching `Tool::run()`.

## Data Types (REQ-AUQ-001, REQ-AUQ-002, REQ-AUQ-003, REQ-AUQ-005)

```rust
// src/state_machine/state.rs

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserQuestion {
    pub question: String,
    pub header: String,
    pub options: Vec<QuestionOption>,
    #[serde(default, rename = "multiSelect")]
    pub multi_select: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionAnnotation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionInput {
    pub questions: Vec<UserQuestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<QuestionMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}
```

Add to `ToolInput` enum:

```rust
AskUserQuestion(AskUserQuestionInput),
```

## State Machine Changes (REQ-AUQ-001, REQ-AUQ-004, REQ-AUQ-007)

### New State: AwaitingUserResponse

The waiting state carries all context needed to resume execution: the tool use
ID (for constructing the tool result), remaining tools (to continue executing
after the response), and persisted tool IDs (to avoid re-persisting results
from tools that completed before the question was asked).

```rust
// src/state_machine/state.rs - add to ConvState enum

AwaitingUserResponse {
    questions: Vec<UserQuestion>,
    tool_use_id: String,
    remaining_tools: Vec<ToolCall>,
    persisted_tool_ids: HashSet<String>,
},
```

### New Events

```rust
AskUserQuestionPending {
    tool_use_id: String,
    questions: Vec<UserQuestion>,
},

UserQuestionResponse {
    answers: HashMap<String, String>,
    annotations: Option<HashMap<String, QuestionAnnotation>>,
},
```

### State Transitions

`ToolExecuting` + `AskUserQuestionPending` transitions to
`AwaitingUserResponse`, persisting state and notifying clients via SSE.

`AwaitingUserResponse` + `UserQuestionResponse` constructs the tool result and
resumes execution: if remaining tools exist, transitions to `ToolExecuting`
for the next tool; otherwise transitions to `LlmRequesting`.

`AwaitingUserResponse` + `UserCancel` constructs an error tool result ("User
declined to answer") and transitions to `Idle`.

## Executor Integration (REQ-AUQ-001, REQ-AUQ-005, REQ-AUQ-006)

The executor intercepts `ask_user_question` before normal tool dispatch.
Sub-agents never see the tool (it is excluded from `ToolRegistry::for_subagent`
per REQ-AUQ-006), so the executor check is defense-in-depth.

Validation (REQ-AUQ-005):
- 1-4 questions, 2-4 options per question
- Question texts unique across the submission
- Option labels unique within each question
- Preview fields only on single-select questions

On validation failure, the tool result is an error with a specific message.
On success, the executor emits `AskUserQuestionPending`.

## Tool Result Format (REQ-AUQ-004)

The tool result delivered to the LLM is a formatted string, not raw JSON:

```
User has answered your questions: "Which library?" = "lodash" [user notes: but only for dates],
"Auth method?" = "OAuth" selected preview:
```oauth2
grant_type=authorization_code
```.
You can now continue with the user's answers in mind.
```

This format includes: the question text, the selected label, any preview content
from the selected option, and any user-added notes. The trailing instruction
reminds the model to incorporate the answers.

## Tool Implementation (REQ-AUQ-001, REQ-AUQ-008)

```rust
// src/tools/ask_user_question.rs

pub struct AskUserQuestionTool;

impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str { "ask_user_question" }

    fn description(&self) -> String {
        "Ask the user clarifying questions when you need input to proceed. \
         Use when there are multiple valid approaches and user preference \
         matters. Provide 1-4 questions with 2-4 options each. Users can \
         also type custom answers."
            .to_string()
    }

    fn input_schema(&self) -> Value { /* see REQ-AUQ-001 schema */ }

    async fn run(&self, _input: Value, _ctx: ToolContext) -> ToolOutput {
        // Executor intercepts -- this is unreachable
        ToolOutput::error("ask_user_question must be handled by executor")
    }
}
```

The tool is registered in all non-sub-agent registries (Explore, Direct,
Work). It is NOT registered in `ToolRegistry::for_subagent()`.

The tool uses `defer_loading: true` via the existing MCP tool search mechanism.
On models without tool search support, it appears in the standard tool list.

## API Endpoint (REQ-AUQ-003, REQ-AUQ-004)

```
POST /api/conversations/{id}/respond
```

Request body:

```json
{
  "answers": { "Which library?": "lodash", "Auth method?": "OAuth" },
  "annotations": {
    "Which library?": { "notes": "but only for dates" },
    "Auth method?": { "preview": "grant_type=authorization_code\n..." }
  }
}
```

The handler sends a `UserQuestionResponse` event to the state machine.
If the conversation is not in `AwaitingUserResponse` state, returns 409
Conflict.

## UI Components (REQ-AUQ-001, REQ-AUQ-002, REQ-AUQ-003, REQ-AUQ-007)

When SSE `state_change` event arrives with `type: "awaiting_user_response"`:

1. Display each question with its header chip and full question text
2. For single-select without previews: radio button list
3. For single-select with previews: side-by-side layout (option list left,
   preview right, updating on focus/hover)
4. For multi-select: checkbox list (no preview support)
5. Each question shows an "Other" text input as the last option
6. Optional notes field per question for user annotations
7. Submit button sends POST to `/api/conversations/{id}/respond`
8. Decline button sends POST to `/api/conversations/{id}/cancel`

## Testing Strategy

### Unit Tests
- Input validation: question/option count constraints, uniqueness checks
- State transitions: all three paths (respond, cancel, sub-agent rejection)
- Tool result formatting: answers with/without previews and notes

### Property Tests
- Arbitrary valid `AskUserQuestionInput` round-trips through serde
- `UserQuestionResponse` with arbitrary answer maps produces valid tool results
- Uniqueness validation rejects inputs with duplicate question/option text

### Integration Tests
- Full flow: tool call -> waiting state -> user response -> LLM continuation
- Cancellation mid-wait
- Multiple questions with mixed single/multi-select
- Preview rendering (visual QA)
