# Ask User Question Tool

## User Story

As an LLM agent, I need to ask the user clarifying questions during a task so that I can make informed decisions that match user preferences rather than guessing.

## Requirements

### REQ-AUQ-001: Question Presentation

WHEN agent needs user input to proceed
THE SYSTEM SHALL allow agent to submit 1-4 structured questions
AND each question SHALL have 2-4 predefined options
AND each option SHALL have a label and optional description

WHEN questions are submitted
THE SYSTEM SHALL pause agent execution
AND display questions to the user with selectable options

**Rationale:** Users benefit from guided choices rather than open-ended prompts. Structured options help users understand their choices and make faster decisions.

---

### REQ-AUQ-002: User Response Collection

WHEN user selects an option for a question
THE SYSTEM SHALL record the selection

WHEN question allows multiple selections
THE SYSTEM SHALL allow user to select multiple options
AND combine selections with comma separator

WHEN user prefers a custom answer
THE SYSTEM SHALL allow free-text input as alternative to predefined options

**Rationale:** Users need flexibility to either quickly choose from options or provide their own answer when none of the options fit.

---

### REQ-AUQ-003: Response Delivery to Agent

WHEN user submits their responses
THE SYSTEM SHALL resume agent execution
AND provide answers as tool result in format `{"answers": {"question text": "selected label"}}`

WHEN user cancels instead of answering
THE SYSTEM SHALL indicate cancellation to agent
AND allow agent to proceed without answers

**Rationale:** Agent needs structured response data to continue the task. Cancellation path prevents users from being stuck.

---

### REQ-AUQ-004: Tool Schema

WHEN LLM requests ask_user_question tool
THE SYSTEM SHALL provide schema with:
- `questions` (required array): 1-4 questions to ask
- Each question has: `question` (string), `header` (short label), `options` (2-4 choices), `multiSelect` (boolean)
- Each option has: `label` (string), `description` (optional string)

**Rationale:** Clear schema enables LLM to construct well-formed questions. Constraints (1-4 questions, 2-4 options) keep interactions focused.

---

### REQ-AUQ-005: Sub-Agent Restriction

WHEN a sub-agent attempts to use ask_user_question
THE SYSTEM SHALL reject the tool call with error
AND indicate that only parent conversations can ask user questions

**Rationale:** Sub-agents run autonomously without direct user interaction. Allowing them to block on user input would break the parallelism model.

---

### REQ-AUQ-006: State Visibility

WHEN agent is waiting for user response
THE SYSTEM SHALL indicate waiting state to connected clients
AND include the questions being asked in state data

WHEN user responds or cancels
THE SYSTEM SHALL transition state and notify clients

**Rationale:** UI needs to know when to show question interface vs normal chat interface.
