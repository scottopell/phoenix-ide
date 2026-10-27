# ADR-155: Slack signals are best effort and questions stay native

- **Status:** Accepted
- **Date:** 2026-10-27
- **Affects:** REQ-SG-005/006/012/013/014/015/016/017

## Context

The owner needs private alerts and an acceptance reaction, while Phoenix owns
work and question interaction. Delivery-proof inspection, acknowledgment retries,
and external question snapshots have created independent recovery and scheduling
obligations disproportionate to these signals' value.

## Options considered

1. Retain ADR-152's receipt inspection and durable acknowledgment retries, with
   continued scheduling and provider-proof maintenance.
2. Remove source-thread replies or thread reuse, substantially changing the
   owner's explicit Slack interaction.
3. Keep native work and intentional source-thread replies durable, but attempt
   notifications and eyes once and publish only a generic clarification link.

## Decision

Choose option 3. Private notices use one durably claimed, bounded command attempt.
Record submitted, failed or uncertain status without automatic retry or receipt
inspection. Submitted means command acceptance, not recipient delivery. Missing
configuration fails visibly. Eyes also has one durably claimed attempt; missing
eyes does not establish that a request was missed. Neither signal blocks an
otherwise settled account change.

Clarifications publish only a generic Phoenix handoff. Native questions, options
and private answers stay in Phoenix; question changes need no external snapshot
replacement. Retained definitely-unsent question-text returns are superseded by
new generic operations; possibly delivered operations remain unchanged.

Thread reuse, exact request coverage, owner verification, durable admission and
truthful uncertain source-thread delivery remain intact. This supersedes
ADR-152's inspection/retry contract and ADR-154's question-text publication.

## Consequences

- **Positive:** No notification inspection scheduler, acknowledgment retry backlog,
  or ongoing question-text synchronization. Ordinary conversations remain durable.
- **Negative:** Alerts and eyes may be missing after failure or interruption;
  question text requires opening Phoenix. There is no automatic signal repair.
- **Neutral:** Version-1 submit-only adapters remain usable. Existing schema history
  stays intact; retained receipts remain historical submission evidence.

## References

- [Slack gateway requirements](../slack-gateway/requirements.md)
- ADR-150, ADR-152 and ADR-154
- `dispatch_returns`, `slack_begin_eyes_attempt`, `record_slack_question_tx`
