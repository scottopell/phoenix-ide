# Ask User Question Tool - Design Document

## Overview

The ask_user_question tool pauses agent execution and waits for user input. Unlike normal tools that execute and return immediately, this tool transitions the conversation to a waiting state until the user responds.

## Tool Interface (REQ-AUQ-004)

### Schema

```json
{
  "type": "object",
  "required": ["questions"],
  "properties": {
    "questions": {
      "type": "array",
      "minItems": 1,
      "maxItems": 4,
      "items": {
        "type": "object",
        "required": ["question", "options"],
        "properties": {
          "question": {
            "type": "string",
            "description": "The full question text to display"
          },
          "header": {
            "type": "string",
            "maxLength": 12,
            "description": "Short label for the question"
          },
          "options": {
            "type": "array",
            "minItems": 2,
            "maxItems": 4,
            "items": {
              "type": "object",
              "required": ["label"],
              "properties": {
                "label": { "type": "string" },
                "description": { "type": "string" }
              }
            }
          },
          "multiSelect": {
            "type": "boolean",
            "default": false
          }
        }
      }
    }
  }
}
```

### Tool Description

```
Ask the user clarifying questions when you need input to proceed.
Use when there are multiple valid approaches and user preference matters.

Provide 1-4 questions with 2-4 options each. Keep questions focused
and options clear. Users can also type custom answers.

NOT available to sub-agents.
```

## Data Types (REQ-AUQ-001, REQ-AUQ-004)

```rust
// src/state_machine/state.rs

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserQuestion {
    pub question: String,
    #[serde(default)]
    pub header: Option<String>,
    pub options: Vec<QuestionOption>,
    #[serde(default, rename = "multiSelect")]
    pub multi_select: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskUserQuestionInput {
    pub questions: Vec<UserQuestion>,
}
```

Add to ToolInput enum:
```rust
AskUserQuestion(AskUserQuestionInput),
```

## State Machine Changes (REQ-AUQ-001, REQ-AUQ-003, REQ-AUQ-006)

### New State: AwaitingUserResponse

```rust
// src/state_machine/state.rs - add to ConvState enum

/// Waiting for user to answer questions (parent conversations only)
AwaitingUserResponse {
    /// The questions being asked
    questions: Vec<UserQuestion>,
    /// Tool use ID (needed to generate tool result)
    tool_use_id: String,
    /// Remaining tools to execute after this one
    remaining_tools: Vec<ToolCall>,
    /// Already persisted tool IDs
    persisted_tool_ids: HashSet<String>,
},
```

### New Events

```rust
// src/state_machine/event.rs

/// ask_user_question tool detected, transitioning to waiting state
AskUserQuestionPending {
    tool_use_id: String,
    questions: Vec<UserQuestion>,
},

/// User has responded to questions
UserQuestionResponse {
    /// Answers keyed by question text
    answers: HashMap<String, String>,
},
```

### State Transitions

```rust
// src/state_machine/transition.rs

// ToolExecuting + AskUserQuestionPending -> AwaitingUserResponse (REQ-AUQ-001, REQ-AUQ-006)
(
    ConvState::ToolExecuting {
        remaining_tools,
        persisted_tool_ids,
        ..
    },
    Event::AskUserQuestionPending {
        tool_use_id,
        questions,
    },
) => Ok(
    TransitionResult::new(ConvState::AwaitingUserResponse {
        questions: questions.clone(),
        tool_use_id,
        remaining_tools: remaining_tools.clone(),
        persisted_tool_ids: persisted_tool_ids.clone(),
    })
    .with_effect(Effect::PersistState)
    .with_effect(Effect::notify_state_change(
        "awaiting_user_response",
        json!({ "questions": questions }),
    )),
),

// AwaitingUserResponse + UserQuestionResponse -> continue (REQ-AUQ-003)
(
    ConvState::AwaitingUserResponse {
        tool_use_id,
        remaining_tools,
        mut persisted_tool_ids,
        ..
    },
    Event::UserQuestionResponse { answers },
) => {
    let result_json = serde_json::json!({ "answers": answers });
    let tool_result = ToolResult::success(tool_use_id.clone(), result_json.to_string());
    persisted_tool_ids.insert(tool_use_id.clone());

    if remaining_tools.is_empty() {
        // No more tools, back to LLM
        Ok(
            TransitionResult::new(ConvState::LlmRequesting { attempt: 1 })
                .with_effect(Effect::PersistToolResults {
                    results: vec![tool_result],
                })
                .with_effect(Effect::PersistState)
                .with_effect(notify_llm_requesting(1))
                .with_effect(Effect::RequestLlm),
        )
    } else {
        // More tools to execute
        let next = remaining_tools[0].clone();
        let rest = remaining_tools[1..].to_vec();

        Ok(
            TransitionResult::new(ConvState::ToolExecuting {
                current_tool: next.clone(),
                remaining_tools: rest,
                persisted_tool_ids,
                pending_sub_agents: vec![],
            })
            .with_effect(Effect::PersistToolResults {
                results: vec![tool_result],
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::execute_tool(next)),
        )
    }
},

// AwaitingUserResponse + UserCancel -> Idle (REQ-AUQ-003)
(ConvState::AwaitingUserResponse { tool_use_id, .. }, Event::UserCancel) => {
    let tool_result = ToolResult::error(
        tool_use_id.clone(),
        "User cancelled the question".to_string(),
    );
    Ok(
        TransitionResult::new(ConvState::Idle)
            .with_effect(Effect::PersistToolResults {
                results: vec![tool_result],
            })
            .with_effect(Effect::PersistState)
            .with_effect(Effect::notify_agent_done()),
    )
},
```

## Executor Integration (REQ-AUQ-001, REQ-AUQ-005)

```rust
// src/runtime/executor.rs

// In execute_tool dispatch, before normal tool execution:
if tool.name() == "ask_user_question" {
    return self.handle_ask_user_question(tool).await;
}

async fn handle_ask_user_question(
    &mut self,
    tool: ToolCall,
) -> Result<Option<Event>, String> {
    // REQ-AUQ-005: Sub-agents cannot ask questions
    if self.context.is_sub_agent {
        return Ok(Some(Event::ToolComplete {
            tool_use_id: tool.id.clone(),
            result: ToolResult::error(
                tool.id,
                "ask_user_question is not available to sub-agents",
            ),
        }));
    }

    // Parse and validate input
    let input: AskUserQuestionInput = match serde_json::from_value(tool.input.to_value()) {
        Ok(i) => i,
        Err(e) => {
            return Ok(Some(Event::ToolComplete {
                tool_use_id: tool.id.clone(),
                result: ToolResult::error(tool.id, format!("Invalid input: {e}")),
            }));
        }
    };

    // Validate constraints
    if input.questions.is_empty() || input.questions.len() > 4 {
        return Ok(Some(Event::ToolComplete {
            tool_use_id: tool.id.clone(),
            result: ToolResult::error(tool.id, "Must have 1-4 questions"),
        }));
    }

    for q in &input.questions {
        if q.options.len() < 2 || q.options.len() > 4 {
            return Ok(Some(Event::ToolComplete {
                tool_use_id: tool.id.clone(),
                result: ToolResult::error(
                    tool.id,
                    format!("Question '{}' must have 2-4 options", q.question),
                ),
            }));
        }
    }

    // Emit pending event to transition state
    Ok(Some(Event::AskUserQuestionPending {
        tool_use_id: tool.id,
        questions: input.questions,
    }))
}
```

## Tool Implementation (REQ-AUQ-004)

The tool struct exists for schema/description purposes only. Execution is handled by executor interception.

```rust
// src/tools/ask_user_question.rs

pub struct AskUserQuestionTool;

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str {
        "ask_user_question"
    }

    fn description(&self) -> String {
        "Ask the user clarifying questions when you need input to proceed. \
         Use when there are multiple valid approaches and user preference matters. \
         Provide 1-4 questions with 2-4 options each. \
         NOT available to sub-agents."
            .to_string()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["questions"],
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "1-4 questions to ask the user",
                    "minItems": 1,
                    "maxItems": 4,
                    "items": {
                        "type": "object",
                        "required": ["question", "options"],
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The full question text"
                            },
                            "header": {
                                "type": "string",
                                "description": "Short label (max 12 chars)"
                            },
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "required": ["label"],
                                    "properties": {
                                        "label": { "type": "string" },
                                        "description": { "type": "string" }
                                    }
                                }
                            },
                            "multiSelect": {
                                "type": "boolean",
                                "default": false
                            }
                        }
                    }
                }
            }
        })
    }

    async fn run(&self, _input: Value, _cancel: CancellationToken) -> ToolOutput {
        // This should never be called - executor intercepts
        ToolOutput::error("ask_user_question must be handled by executor")
    }
}
```

## API Endpoint (REQ-AUQ-002, REQ-AUQ-003)

```rust
// src/api/routes.rs

#[derive(Deserialize)]
struct UserQuestionResponsePayload {
    answers: HashMap<String, String>,
}

/// POST /conversations/{id}/respond
async fn respond_to_question(
    State(manager): State<Arc<RuntimeManager>>,
    Path(conversation_id): Path<String>,
    Json(payload): Json<UserQuestionResponsePayload>,
) -> impl IntoResponse {
    manager
        .send_event(
            &conversation_id,
            Event::UserQuestionResponse {
                answers: payload.answers,
            },
        )
        .await
        .map(|_| StatusCode::OK)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))
}
```

## UI Components (REQ-AUQ-001, REQ-AUQ-002, REQ-AUQ-006)

When SSE state_change event arrives with `type: "awaiting_user_response"`:

1. Display each question with its header and full text
2. Show options as radio buttons (single-select) or checkboxes (multi-select)
3. Include "Other" option with text input for free-text responses
4. Submit button sends POST to `/conversations/{id}/respond`
5. Cancel button sends POST to `/conversations/{id}/cancel`

## Tool Registry (REQ-AUQ-004, REQ-AUQ-005)

```rust
// src/tools.rs - in ToolRegistry::new_with_options

if !is_sub_agent {
    // Parent conversations can ask questions and spawn sub-agents
    tools.push(Arc::new(AskUserQuestionTool));
    tools.push(Arc::new(SpawnAgentsTool));
}
```

## Testing Strategy

### Unit Tests
- Input validation (question/option counts)
- State transitions (all paths)
- Tool result formatting

### Integration Tests
- Full flow: tool call -> waiting state -> user response -> continuation
- Sub-agent rejection
- Cancellation path
- Multiple questions with mixed single/multi-select

### Property Tests
```rust
#[proptest]
fn answers_always_valid_json(answers: HashMap<String, String>) {
    let result = serde_json::json!({ "answers": answers });
    assert!(serde_json::to_string(&result).is_ok());
}
```

## File Organization

```
src/
├── tools/
│   ├── mod.rs                  # Add AskUserQuestionTool export
│   └── ask_user_question.rs    # Tool implementation (NEW)
├── state_machine/
│   ├── state.rs                # Add AwaitingUserResponse, types
│   ├── event.rs                # Add events
│   └── transition.rs           # Add transitions
├── runtime/
│   └── executor.rs             # Add interception logic
└── api/
    └── routes.rs               # Add /respond endpoint
```
