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
AND start every question unanswered, without interpreting a recommendation as
selection or consent

WHEN the user navigates between questions
THE SYSTEM SHALL retain per-question answers, notes, custom drafts, and preview
disclosure within the pending request
AND distinguish answered questions from merely visited questions
AND allow navigation to unanswered questions

WHEN the user advances with Next
THE SYSTEM SHALL require the current question to be answered without sending

WHEN the user reaches the final question
THE SYSTEM SHALL label the action Send answer for one question or Send answers
for multiple questions
AND enable it only when every question is answered
AND identify missing answers with navigation and visible explanatory text

**Rationale:** Users make faster, better decisions when presented with guided
choices rather than open-ended prompts. Structured options help users understand
the trade-offs without needing to formulate their own alternatives.

---

### REQ-AUQ-002: Rich Option Previews

WHERE a question presents concrete artifacts that benefit from visual comparison
(code snippets, configuration examples, layout mockups)
THE SYSTEM SHALL support an optional preview field on each option
AND render the selected option's preview with a heading naming that option

WHEN any option in a question has a preview
THE SYSTEM SHALL derive the displayed preview only from the selected option
AND SHALL NOT change it on pointer hover or unselected keyboard focus
AND use the responsive presentation defined by REQ-AUQ-009

WHEN no option is selected in a preview-capable question
THE SYSTEM SHALL show Choose an option to view its preview

WHEN the selected option has no preview
THE SYSTEM SHALL show No preview for this option without reusing other content

WHEN Other is selected in a preview-capable question
THE SYSTEM SHALL keep the custom editor in the answer column
AND show Your custom answer will be sent in the preview region without
duplicating the editable text

WHEN no options have previews
THE SYSTEM SHALL display options in standard list layout

IF a question uses multiSelect
THE SYSTEM SHALL NOT render previews for that question (previews require
single-select comparison)

**Rationale:** Comparing code snippets or configuration examples requires
seeing the actual content, not just a label. Side-by-side preview eliminates
the mental overhead of reading option descriptions and imagining the output.

---

### REQ-AUQ-003: Flexible Response Collection

WHEN user selects an option for a single-select question
THE SYSTEM SHALL record the selected option's label as the answer
AND SHALL NOT advance, send, or focus notes as a consequence of selection

WHEN user selects options for a multi-select question
THE SYSTEM SHALL record all selected option labels as a comma-separated answer
in displayed order, followed by any included custom answer

WHEN user prefers a custom answer
THE SYSTEM SHALL always provide an "Other" option with a free-text input field
AND agent-provided options SHALL NOT include an "Other" option (the system adds
it automatically)

WHEN user adds notes to their selection
THE SYSTEM SHALL record the notes as an annotation alongside the answer

THE SYSTEM SHALL offer optional per-question notes for every question and answer
mode, including Other and multi-select
AND SHALL NOT treat notes alone as an answer
AND SHALL preserve notes when the user changes choices or collapses the editor
AND label collapsed nonempty notes Edit notes · included

WHEN optional notes are expanded
THE SYSTEM SHALL focus their textarea
AND clearing notes SHALL require explicit editing

WHEN Other is chosen
THE SYSTEM SHALL reveal a multiline editor directly below its choice
AND focus it on pointer activation or explicit Edit
AND leave radio focus unchanged on arrow-key selection so Tab enters the editor

WHEN the user focuses, clicks, or repositions the caret in the custom editor
THE SYSTEM SHALL NOT toggle Other off

WHEN non-whitespace custom text is entered
THE SYSTEM SHALL select Other without clearing other multi-select choices

WHEN Other is explicitly deselected or a single-select choice replaces it
THE SYSTEM SHALL retain the custom draft but exclude it from the answer
AND visibly identify the saved draft as not included
AND provide Edit to select Other and reopen its editor

WHEN Other is selected with whitespace-only text
THE SYSTEM SHALL identify the missing custom answer beside the editor
AND SHALL NOT allow completion, even when other multi-select choices are checked

WHEN answers are serialized
THE SYSTEM SHALL trim only outer whitespace from custom answers
AND preserve internal whitespace, newlines, and user-authored notes without
summarization, truncation, or an additional character limit
AND derive preview annotation from the selected option

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
AND include the questions and originating tool_use_id in the state data

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

### REQ-AUQ-009: Responsive Reading and Reachable Actions

THE SYSTEM SHALL attach the panel to the conversation with a compact context
header, one vertically scrolling answer body, and reachable action footer
AND retain conversation identity and application navigation
AND show the full question in the body while its compact header remains visible
AND omit question-progress navigation for a single question
AND show the current question number, set size, and short header with compact
navigation controls for multiple questions

WHERE the panel is at least 840 CSS px wide
THE SYSTEM SHALL present preview questions in an approximately 60/40 split with
at least 420 px for choices, 320 px for preview, and a 24 px gap
AND cap centered content at 1200 px and ordinary answer text at 80ch

WHERE the panel is narrower than 840 CSS px
THE SYSTEM SHALL use one column with the selected preview after all choices
and before notes
AND provide Preview below on the selected option to scroll to and focus its
preview heading without moving the choice rows

THE SYSTEM SHALL use 16 px outer padding below 480 CSS px and 24 px otherwise
AND show complete wrapping option descriptions without ellipsis
AND prevent horizontal page overflow at 320 CSS px and 400% desktop zoom

WHERE panel width is below 480 CSS px
THE SYSTEM SHALL use the full available conversation content region

WHERE panel width is at least 480 CSS px
THE SYSTEM SHALL size the panel to content up to 65% of available conversation
height and offer Expand questions and Restore conversation as a local toggle

WHEN a preview is longer than eight displayed lines
THE SYSTEM SHALL offer inline Show full preview and Show less
AND wrap normal preview text while allowing horizontal scrolling only within
code blocks
AND use the answer body's vertical scroller for expanded content

WHERE a wide-layout preview fits the usable body height
THE SYSTEM SHALL keep it sticky within that body

WHERE a preview exceeds usable body height
THE SYSTEM SHALL keep it in normal flow

WHILE the software keyboard is visible
THE SYSTEM SHALL use the visible viewport to keep the active editor and caret
visible and permit the answer body to shrink and scroll
AND SHALL NOT scroll the outer transcript behind the panel

WHERE usable height is below 360 CSS px
THE SYSTEM SHALL compact the context header to one line and place the footer in
flow so the editor and actions remain reachable

THE SYSTEM SHALL support a two-row footer when necessary, safe-area padding,
44 by 44 CSS px effective touch targets, and at least 16 px editable text on phones
AND use Phoenix visual tokens, at least 14 px descriptions and 16 px labels at
normal scale, an 8 px spacing rhythm, and normal body contrast
AND distinguish native checked selection, visible keyboard focus, errors, and
completion without relying on color alone
AND SHALL NOT introduce resizable splitters, detached preview windows, decorative
section cards, animated automatic advancement, or disappearing descriptions

**Rationale:** Users need to compare complete choices and reach the answer
controls in narrow panes, enlarged text, and keyboard-reduced viewports.

---

### REQ-AUQ-010: Accessible Native Interaction

THE SYSTEM SHALL follow the focus-scope contract in
[Keyboard Interaction Model](../keyboard-interaction/requirements.md)
AND use labeled native radios or checkboxes with associated descriptions and a
labeled choice group
AND place editable fields outside choice labels
AND expose historical questions and answers as static content without edit/send
shortcuts or active-looking disabled form controls

WHEN the panel appears or the question changes
THE SYSTEM SHALL focus the selected choice or the first choice if unanswered
without implicitly selecting it
AND scroll focused controls into view with nearest alignment without animation

THE SYSTEM SHALL preserve native Tab, Shift+Tab, radio arrows, and checkbox Space
AND SHALL NOT use Tab to change questions or Enter to time an advance or submit
AND SHALL keep Enter as a newline inside editors and activation on buttons

WHEN Cmd/Ctrl+Enter is pressed outside IME composition in the eligible AUQ scope
THE SYSTEM SHALL send the completed set explicitly
OR focus the first missing answer and announce why sending is unavailable

WHEN n is pressed outside editable controls in the eligible AUQ scope
THE SYSTEM SHALL open or refocus optional notes without closing them

WHEN Escape is pressed in an editor
THE SYSTEM SHALL return focus to its disclosure or Other control without loss

WHEN Escape is pressed in an open sub-context
THE SYSTEM SHALL close that context before offering dismissal confirmation

WHEN Escape is pressed in dismissal confirmation
THE SYSTEM SHALL cancel that confirmation

THE SYSTEM SHALL announce status changes politely and errors with a stable
visible message and alert
AND SHALL NOT announce every keystroke or move focus on hover
AND document shortcuts in global help
AND respect reduced motion, high contrast, 200% text scaling, screen readers,
and WCAG 2.2 AA contrast and focus visibility

**Rationale:** Standard controls allow keyboard and assistive-technology users
to answer predictably without accidental sending or hidden focus.

---

### REQ-AUQ-011: Request-Bound Responses and Dismissal

THE SYSTEM SHALL identify a pending question request by its conversation ID and
originating tool_use_id
AND require that identity in response and dismissal payloads across supported
web, CLI, and native iOS callers
AND carry the identity through runtime events
AND compare it both at admission and when the state transition consumes the
pending request

WHEN identity is missing or no longer matches the pending request
THE SYSTEM SHALL reject the mutation without consuming another request
AND provide an actionable reload/update or stale-request error
AND SHALL NOT accept an identity-free compatibility fallback

WHEN a request is consumed
THE SYSTEM SHALL consume it and authorize any response-driven resume once through
the state-machine commit boundary

WHEN a response or dismissal completes asynchronously
THE SYSTEM SHALL apply success, error, focus restoration, reset, and control
re-enabling only to the originating request
AND leave a newer request's panel, draft, and focus intact

**Rationale:** Identical question text does not authorize a stale client to
answer a different request. Late responses must not erase the next question.

**Dependencies:** [Compatibility Guarantees](../compatibility/requirements.md).

---

### REQ-AUQ-012: Truthful Sending and Uncertain Outcomes

WHEN the user sends an answer or confirms dismissal
THE SYSTEM SHALL freeze that operation's snapshot and permit one client mutation
request in flight
AND disable answer editing, question navigation, competing dismissal/send, and
repeated sending while that request is unresolved

WHEN sending is confirmed successful for the displayed request
THE SYSTEM SHALL close it, restore composer focus, announce Answers sent, and
reconcile authoritative state

WHEN the user chooses Use chat instead
THE SYSTEM SHALL confirm that no answer will be sent and the agent will wait for
a message, with Keep answering and Use chat instead actions

WHEN dismissal is confirmed successful for the displayed request
THE SYSTEM SHALL close it, focus the composer, and announce that questions were
dismissed and a message is needed to continue

WHEN a typed rejection guarantees an attempt did not mutate
AND no earlier attempt remains uncertain
THE SYSTEM SHALL preserve drafts, restore editing, and show the actionable error
without closing or claiming success

WHEN an outcome is uncertain, including transport failure or generic server error
THE SYSTEM SHALL keep the original operation frozen and check authoritative state
AND SHALL NOT automatically retry a mutation or claim that the answer was saved
or sent

WHEN reconciliation shows the same request still pending
THE SYSTEM SHALL explain that the original operation may still be processing
AND offer only explicit Retry same answer with the identical frozen snapshot,
or Retry dismissal with the same identity for an uncertain dismissal
AND SHALL NOT allow a competing edited answer or dismissal/send operation

WHEN a retry is rejected without mutation
THE SYSTEM SHALL retain uncertainty about any earlier attempt

WHILE any attempt remains uncertain
THE SYSTEM SHALL keep editing locked until authoritative resolution or proof
that every outstanding attempt cannot mutate

WHEN reconciliation fails
THE SYSTEM SHALL preserve the frozen operation and offer Check status again

WHEN authoritative state shows the originating request is no longer pending
THE SYSTEM SHALL remove its stale editing surface and announce that it is no
longer awaiting an answer without claiming that this device sent it

**Rationale:** A state read can race an earlier mutation. Retrying a frozen
snapshot preserves the user's intended answer without requiring a receipt store
or allowing a newer edit to lose to an older request.

---

### REQ-AUQ-013: Draft Lifetime and Request Isolation

WHEN the same pending request reconnects while its panel remains mounted
THE SYSTEM SHALL retain its in-memory draft and reconcile waiting state without
resetting that draft

WHEN the pending request identity or conversation changes
THE SYSTEM SHALL start a separate unanswered draft, even when question text
matches

WHEN notes or custom text first becomes nonempty
THE SYSTEM SHALL explain concisely that refreshing or leaving can discard the
in-memory draft

THE SYSTEM SHALL allow leaving during unresolved operation uncertainty
AND SHALL NOT promise draft recovery after leaving or refreshing, offline draft
synchronization, or revision/cancellation of an uncertain submitted operation

**Rationale:** Users must know the lifetime of their unsent text without
mistaking a local draft for durable or cross-device state.
