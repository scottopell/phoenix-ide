# Ask User Question Tool

## User Story

As an LLM agent working on a complex task, I need to ask the user clarifying
questions when multiple valid approaches exist so that I make informed decisions
that match user preferences rather than guessing.

As a user, I want the agent to pause and ask me when it's uncertain, showing me
clear options I can choose from quickly, so I stay in control without
micromanaging every step.

## Requirements

### REQ-AUQ-001: Structured Question Presentation

WHEN agent needs user input to proceed
THE SYSTEM SHALL allow agent to submit 1-4 structured questions
AND each question SHALL have a short header label (12 characters max), full
question text, and 2-4 predefined options
AND each option SHALL have a concise label (1-5 words) and an optional
description explaining trade-offs or implications

WHEN questions are submitted
THE SYSTEM SHALL pause agent execution
AND display all questions to the user with selectable options

**Rationale:** Users make faster, better decisions when presented with guided
choices rather than open-ended prompts. Structured options help users understand
the trade-offs without needing to formulate their own alternatives.

---

### REQ-AUQ-002: Rich Option Previews

WHERE a question presents concrete artifacts that benefit from visual comparison
(code snippets, configuration examples, layout mockups)
THE SYSTEM SHALL support an optional preview field on each option
AND render preview content in a monospace box alongside the option

WHEN any option in a question has a preview
THE SYSTEM SHALL display a side-by-side layout with the option list on one side
and the focused option's preview on the other

WHEN no options have previews
THE SYSTEM SHALL display options in standard list layout

IF a question uses multiSelect
THE SYSTEM SHALL NOT render previews for that question (previews require
single-select focus interaction)

**Rationale:** Comparing code snippets or configuration examples requires
seeing the actual content, not just a label. Side-by-side preview eliminates
the mental overhead of reading option descriptions and imagining the output.

---

### REQ-AUQ-003: Flexible Response Collection

WHEN user selects an option for a single-select question
THE SYSTEM SHALL record the selected option's label as the answer

WHEN user selects options for a multi-select question
THE SYSTEM SHALL record all selected option labels as a comma-separated answer

WHEN user prefers a custom answer
THE SYSTEM SHALL always provide an "Other" option with a free-text input field
AND agent-provided options SHALL NOT include an "Other" option (the system adds
it automatically)

WHEN user adds notes to their selection
THE SYSTEM SHALL record the notes as an annotation alongside the answer

**Rationale:** Users need flexibility to quickly choose from options or provide
their own answer when none fit. The automatic "Other" option ensures the user is
never trapped by insufficient choices. Annotations let users add context
("this approach, but with X modification") without losing the structure of the
selection.

---

### REQ-AUQ-004: Response Delivery to Agent

WHEN user submits their responses
THE SYSTEM SHALL resume agent execution
AND provide answers as a formatted tool result that maps each question text to
the selected label, including any preview content of the selected option and
any user-added notes

WHEN user declines to answer
THE SYSTEM SHALL indicate the refusal to the agent
AND allow the agent to proceed using its own judgment

**Rationale:** The agent needs structured response data to continue the task.
Including preview content and notes in the result gives the agent full context
about what the user chose and why. The decline path prevents users from being
stuck if they don't want to answer.

---

### REQ-AUQ-005: Prevent Ambiguous Question Responses

THE SYSTEM SHALL reject questions with duplicate question text across the
submitted set
THE SYSTEM SHALL reject questions where option labels are not unique within
a single question
THE SYSTEM SHALL reject submissions with fewer than 1 or more than 4 questions
THE SYSTEM SHALL reject questions with fewer than 2 or more than 4 options

WHEN validation fails
THE SYSTEM SHALL return the validation error to the agent as a tool error result
AND allow the agent to retry with corrected input

**Rationale:** Duplicate questions or options create ambiguous responses where
the system cannot determine which question or option the user meant. Enforcing
constraints at submission time produces clear error feedback rather than
confusing UI behavior.

---

### REQ-AUQ-006: Parent Conversation Availability

THE SYSTEM SHALL register the question tool only in parent conversation tool
registries (Explore, Standalone, Work modes)

THE SYSTEM SHALL NOT register the question tool in sub-agent tool registries

**Rationale:** Sub-agents are invisible background workers with no direct user
interaction surface. A question from a sub-agent would have no UI to display in
and no user watching to answer it. Only the parent conversation has an active
user session.

---

### REQ-AUQ-007: Real-Time Waiting Feedback

WHEN agent is waiting for user response
THE SYSTEM SHALL indicate the waiting state to connected clients
AND include the questions being asked in the state data

WHEN user responds or declines
THE SYSTEM SHALL transition state and notify all connected clients

**Rationale:** Users need immediate visual feedback that the agent is waiting
for their input, not stuck or working. Seeing the questions appear in real time
confirms the agent heard them and is ready for their decision.

---

### REQ-AUQ-008: Low-Overhead Tool Availability

THE SYSTEM SHALL mark the question tool as deferred for tool search on models
that support it

WHEN the model does not support tool search
THE SYSTEM SHALL include the tool in the standard tool list

**Rationale:** The question tool is used infrequently relative to core tools
like bash and patch. Deferring it via tool search reduces context token cost
without impacting availability -- the model discovers it when it needs to ask
a question.
