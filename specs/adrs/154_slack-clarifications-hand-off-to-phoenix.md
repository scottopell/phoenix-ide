# ADR-154: Slack clarifications hand off to Phoenix

- **Status:** Accepted; question-text publication superseded by ADR-155
- **Date:** 2026-10-26
- **Affects:** REQ-SG-012, REQ-SG-013

## Context

A thread can contain a pending agent question and an unrelated later request.
Message ordering does not establish that the owner is answering the question.
Phoenix already provides exact question identity and an ordinary answer UI.
The owner prefers a simple initial experience with deep links into Phoenix.

## Options considered

1. Infer answers from later fire triggers: convenient, but an unrelated task
   can become an answer or apparent approval.
2. Add explicit question-reference syntax in Slack: unambiguous, but introduces
   another interaction contract and answer mapping surface.
3. Link clarifications to Phoenix: reuse its existing answer interaction and
   make the transition into private discussion explicit.

## Decision

Choose the Phoenix handoff. Post the clarification with a stable conversation
deep link and instructions to answer there. Its confirmed delivery satisfies
the original automatic Slack response obligation. Later Phoenix discussion is
private; a new explicit Slack request can request another intentional reply.
Slack triggers use ordinary admission and never submit question responses.
Reject any unsubmitted persisted Slack-to-question association rather than
trust the old ordering inference. Keep the immutable feature migration intact.

## Consequences

- No extra answer syntax, permission mode or question-response runtime.
- Clarifications require opening Phoenix; Slack-only question answers are
  unsupported.
- Pending questions can block ordinary admission; the rejection explains the
  handoff and links to the same Phoenix conversation.
- Gateway-generated clarifications and explanations persist their full link
  with their outgoing text, preserving known-unsent retry identity.

## References

- [Slack gateway requirements](../slack-gateway/requirements.md)
- `accept_slack_source`, `process_slack_native_outcomes`, `admit`
- [Native question authority](../ask-user-question/requirements.md)
