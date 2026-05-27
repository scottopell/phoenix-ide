# Remove AUQ decline-as-agent-instruction behavior

## Problem

`ask_user_question` currently treats the UI's **Decline** action as an implicit user message to the agent:

> I declined to answer those questions. Please proceed using your own judgment.

That is the wrong semantic contract. A user declining the structured questionnaire is not the same as authorizing the agent to continue without input. It often means: "I don't want to answer in this rigid form; I want to type what I mean in chat instead."

The current behavior is also encoded in the AUQ spec:

- `specs/ask-user-question/requirements.md` says decline allows the agent to proceed using its own judgment.
- `specs/ask-user-question/design.md` says cancel constructs a decline result / posts to cancel.
- `QuestionPanel.tsx` labels the action "Decline" and confirms with "The agent will proceed using its own judgment."
- `transition.rs` turns `AwaitingUserResponse + UserCancel` into a persisted user message plus `RequestLlm`.

## Desired behavior

Remove the "decline answers and proceed with best judgment" path for `ask_user_question`.

Instead, the AUQ panel should support a dismissal/escape hatch whose meaning is purely UI/control-flow:

- Dismisses the structured question panel.
- Does **not** send "proceed with your own judgment" or equivalent content to the agent.
- Does **not** trigger an LLM continuation on its own.
- Leaves the user able to type a normal chat message to clarify, reframe, or answer in free-form prose.

## Proposed implementation plan

1. **Update AUQ spec language**
   - Replace the REQ-AUQ-004 decline requirement with a dismiss/reframe requirement.
   - Clarify that dismissal is not an answer and not authorization to proceed autonomously.
   - Update REQ-AUQ-007 and design.md references from "responds or declines" / `Decline` to "responds or dismisses".

2. **Model dismissal explicitly**
   - Avoid overloading generic `UserCancel` semantics if possible; `UserCancel` still means abort active LLM/tool/sub-agent work elsewhere.
   - Add an AUQ-specific event or parent event such as `UserQuestionDismissed` / `DismissUserQuestion`.
   - Transition `AwaitingUserResponse + dismiss` to a state where the conversation can accept a normal user message without requesting the LLM immediately.
   - Preserve enough state/display data so the UI no longer shows the structured panel, but the conversation is not silently continued.

3. **Allow free-form chat after dismissal**
   - Today `AwaitingUserResponse + UserMessage` is rejected with `TransitionError::AwaitingUserResponse`.
   - After dismissal, typing a chat message should work normally and be the explicit user input that resumes the agent.
   - Add/update tests for this path.

4. **Update UI copy and controls**
   - Rename the button from **Decline** to **Dismiss** or similar.
   - Confirmation copy should say the structured questions will be closed and the user can type a message instead.
   - Remove copy/tooltips/shortcut help that imply declining answers the agent or authorizes autonomous continuation.
   - Escape should dismiss/confirm dismissal, not "decline".

5. **Update API client/server endpoints as needed**
   - Either add a dedicated AUQ dismiss endpoint or make the existing endpoint send the new AUQ-specific event only when in `AwaitingUserResponse`.
   - Avoid changing global `/cancel` behavior in a way that breaks cancellation of running LLM/tool/sub-agent work.

6. **Regression tests**
   - State-machine test: AUQ dismissal does not persist a "proceed using your own judgment" message.
   - State-machine test: AUQ dismissal does not emit `RequestLlm`.
   - State-machine/API/UI test as appropriate: after dismissal, a normal user message is accepted and resumes the agent.
   - UI test/copy assertion for Dismiss wording if existing component tests cover QuestionPanel.

## Acceptance criteria

- There is no user-facing **Decline** action in the AUQ panel.
- No code path for AUQ dismissal persists or sends "proceed with your own judgment" / "best judgment" to the agent.
- Dismissing AUQ is not treated as an answer.
- Dismissing AUQ does not trigger an LLM request by itself.
- After dismissal, the user can type a normal message to continue the conversation.
- Specs and tests reflect the new contract.
