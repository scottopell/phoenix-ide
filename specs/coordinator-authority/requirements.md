# Coordinator Authority

## User Story

As a Phoenix user, I want Ember to retain clear standing approvals and operating
boundaries so that it can carry out authorized work without repeatedly asking
me to approve the same class of action. I want to describe my intent in normal
conversation, have Ember turn it into precise operating instructions, and
approve that wording before Ember saves it.

## Requirements

### REQ-AUTH-001: Retain One Instance-Wide Authority Document

THE SYSTEM SHALL retain one current natural-language Authority document for the
local Global Coordinator, independently of transcript identity and continuation.

THE document SHALL be read and replaced as one indivisible text value.

WHEN no Authority document has been saved, or the document is cleared
THE SYSTEM SHALL represent the absence of additional standing grants explicitly
AND SHALL NOT infer a grant from transcript history, summaries or retained files.

THE SYSTEM SHALL NOT initialize Authority by importing historical approvals.

### REQ-AUTH-002: Read and Replace Through Current-Coordinator Tools

THE SYSTEM SHALL provide local `read_authority` and `set_authority` tools only
to the current singleton Global Coordinator.

THE SYSTEM SHALL derive the source transcript from trusted tool execution
context and validate it against the persisted current Coordinator
AND SHALL revalidate that source within the committing write transaction.

THE SYSTEM SHALL refuse ordinary conversations, sub-agents and predecessor
Coordinator transcripts, including an earlier source retaining a tool handle.

`read_authority` SHALL accept an empty object and return the current instructions.

`set_authority` SHALL accept exactly one required string field, `instructions`,
and replace the entire document with that text. An empty string SHALL clear it.

THE tools SHALL reject unknown fields, supplied source or target identities,
remote selectors and incorrectly typed arguments.

THE SYSTEM SHALL accept at most 16,384 UTF-8 bytes of instructions
AND SHALL reject a larger document without changing the saved value
AND SHALL NOT silently clip, summarize or partially apply the replacement.

### REQ-AUTH-003: Obtain Conversational Approval Before Saving

WHEN user feedback implies a change to standing Authority
THE Coordinator instructions SHALL direct the agent to read current Authority,
interpret the feedback into clear scoped operating rules, and clarify material
ambiguity before presenting the complete proposed replacement for user approval.

THE instructions SHALL require the agent to obtain user approval of that
wording in ordinary conversation before calling `set_authority`
AND SHALL require renewed approval when the proposed wording materially changes.

THE instructions SHALL distinguish standing permission from a one-time approval
AND SHALL prohibit promoting summaries, quotations, delegate claims or historical
tool output into a new user grant.

THE instructions SHALL permit the user to deliberately grant broad authority
AND SHALL prohibit the agent from inventing such a grant.

THE SYSTEM SHALL describe this approval requirement as agent instruction,
not as server-verified proof of human approval.

THE setter SHALL NOT require a synthetic approval flag, a persisted proposal,
a review-card action or a dedicated conversation waiting state.

### REQ-AUTH-004: Supply Current Authority in Model Context

WHEN the Coordinator dispatches an ordinary model request, including a request
admitted from a watch event or after continuation
THE SYSTEM SHALL read the current saved document and include its complete text
as authoritative operating context.

THE context SHALL identify an empty document as no additional standing grants
AND SHALL distinguish this server-generated projection from historical database,
transcript, summary and file contents.

THE prompt-inspection surface SHALL use the same Authority renderer as live
request construction.

WHILE the saved document is unchanged
THE rendered Authority context SHALL be byte-identical across requests
AND SHALL NOT append changing timestamps or activity facts.

IF reading current Authority fails
THE SYSTEM SHALL surface the failure and SHALL NOT dispatch with a silently
omitted, empty-substituted or stale Authority document.

### REQ-AUTH-005: Establish Replacement Through Normal Persistence Ownership

WHEN `set_authority` succeeds
THE SYSTEM SHALL establish and return the exact replacement within its local
persistence command boundary before reporting success.

THE command SHALL preserve the established local persistence classification and
owned-write rules in [Compatibility](../compatibility/requirements.md)
AND SHALL NOT report an unclassified write as ordinary success or refusal.

THE saved document SHALL be available to subsequent model requests after
restart and continuation without depending on a transcript summary or a live
runtime cache.

THE change SHALL affect subsequent model requests and SHALL NOT retroactively
cancel a tool action already admitted under its existing execution authority.

### REQ-AUTH-006: Apply Grants Within Existing Execution Boundaries

THE Coordinator instructions SHALL direct the agent to perform in-scope work
under saved standing grants without redundant permission requests, while
checking the facts needed to establish that a contemplated action fits them.

THE instructions SHALL preserve later user restrictions, pauses and Stop
instructions and SHALL NOT treat the agent's own reports as changes to the
user's standing grants.

Authority SHALL NOT override deterministic tool denials, WorkScope targeting,
destination authorization or other structural capability boundaries.

Authority SHALL NOT itself commission new work, schedule a turn, resume paused
execution, supply authority to ordinary conversations or synchronize grants to
another Phoenix instance.

## Scope

This feature is local document persistence, Coordinator-only read/set tools and
model context. Conversational approval uses the normal conversation and any
question tool available there. Proposal histories, revision protocols, approval
UI, semantic action enforcement, pending user interactions, timers and automatic
continuation from unanswered questions are separate concerns.
