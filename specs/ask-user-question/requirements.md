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

WHEN user dismisses the structured question panel
THE SYSTEM SHALL close the panel without recording an answer
AND SHALL NOT indicate any refusal to the agent
AND SHALL NOT authorize the agent to proceed autonomously
AND SHALL NOT resume agent execution until the user sends an explicit message

**Rationale:** The agent needs structured response data to continue the task.
Including preview content and notes in the result gives the agent full context
about what the user chose and why. Dismissal is an escape hatch from the
structured UI, not an answer; users can reframe or answer in normal chat prose.

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

THE SYSTEM SHALL register the question tool only in parent-conversation tool
registries

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

WHEN user responds or dismisses
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

---

### REQ-AUQ-009: Bind Actions to the Current Pending Question

WHEN the system creates a pending structured-question request
THE SYSTEM SHALL assign a Phoenix-owned identity independent of the provider tool-use identity
AND SHALL persist that identity with the pending conversation state

WHEN a client answers or dismisses an identified pending request
THE SYSTEM SHALL require the submitted identity to exactly match the current pending identity
AND SHALL reject a missing or stale identity with an actionable conflict response
AND SHALL validate the identity while holding the conversation's mutation-admission lock

WHERE a pending request predates question-request identities
THE SYSTEM SHALL preserve the absent identity
AND SHALL accept only a correspondingly identity-absent answer or dismissal

WHEN a newer pending request replaces the question displayed by a client
THE client SHALL discard local question input for the previous identity
AND a completion from the previous request SHALL NOT close or advance the newer question panel

---

### REQ-AUQ-010: Coordinator Questions Do Not Block Coordination

THE Coordinator SHALL ask the user structured questions with a non-blocking
tool that records the question and returns a request identity at once
AND SHALL NOT be offered the blocking question tool.

WHILE a Coordinator question is open
THE Coordinator SHALL keep accepting user messages and watch events and keep
coordinating
AND its prompt SHALL direct it not to act on what the question asks until the
reply arrives.

A question SHALL belong to the Coordinator's stable conversation identity, not
to the transcript that asked it.

THE question SHALL stay open until the user answers or dismisses it or the
Coordinator withdraws it, across Coordinator turns, continuations, restarts and
deletion of the transcript that asked it.

IF the Coordinator conversation itself is deleted
THEN its open questions SHALL close as withdrawn.

Ordinary conversations SHALL keep the blocking question tool unchanged.

**Rationale:** The Coordinator serves many conversations. Parking all of them
behind one question to the user stalls every worker it watches; the user may
not answer for hours.

---

### REQ-AUQ-011: Answer Coordinator Questions Outside the Transcript

THE SYSTEM SHALL list open Coordinator questions to clients, oldest first.

WHEN Ember's page is open and a Coordinator question is open
THE SYSTEM SHALL show the oldest question with the standard question controls,
how long ago it was asked, and how many more wait, without replacing the
composer.

WHILE any Coordinator question is open
THE Ember entry SHALL show how many questions wait for the user.

WHEN a Coordinator question opens while the app is running
THE SYSTEM SHALL raise one question notification for it, under the user's
question-notification setting. Questions already open when the app loads SHALL
NOT be announced again. Activating the notification SHALL open Ember's page on
its current transcript. No notification SHALL be raised while Ember's page is
visible and focused, whichever Ember transcript it shows.

WHEN the user answers or dismisses a Coordinator question
THE SYSTEM SHALL accept only the first reply, refusing later ones as stale,
AND SHALL deliver the reply to the current Coordinator transcript as a user
message naming the request identity, retrying until accepted even if the
Coordinator is busy or between transcripts.

THE reply SHALL count as delivered in the same transaction in which a
transcript of that Coordinator accepts it, directly or as queued steering, so
no later Coordinator transcript receives it again. A reply accepted and then
cancelled before the Coordinator reads it SHALL NOT be resent.

**Rationale:** Answers are the user's words and must reach the Coordinator even
when it is mid-turn; storing the reply before delivery means no answer is lost.
Message-id deduplication is per transcript, so delivery must be recorded at
acceptance rather than afterwards, or a continuation between the two steps
would receive the reply a second time.

---

### REQ-AUQ-012: Withdraw and List Coordinator Questions

WHEN the Coordinator settles an open question another way
THE Coordinator SHALL be able to withdraw it, which closes it without a reply
AND SHALL be refused once the user has answered or dismissed it.

THE Coordinator SHALL be able to list its open questions with their request
identities, oldest first,
AND its prompt SHALL direct it to check that list before asking and after a
continuation, so it does not ask again what is already open.

**Rationale:** A question that no longer matters should not wait for the user.
