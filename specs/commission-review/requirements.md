# Commission Review Unavailability

## User Story

As a Phoenix user, I want commission review to be absent from agent tools and approval surfaces so that it cannot consume review tokens, compete with ordinary pull-request review, or park a conversation behind an unavailable action.

As a Phoenix user with existing conversation history, I want historical tool calls to remain readable and unsupported pending approvals to recover safely so that transcripts remain valid and conversations remain usable.

## Requirements

### REQ-CR-016: Commission Review Has No Product Authority

THE SYSTEM SHALL NOT advertise, approve, execute, or resume `commission_review`

AND SHALL NOT expose an internal approved alias, specialized approval endpoint, or specialized result viewer for that capability.

IF a stale model response requests `commission_review`
THE SYSTEM SHALL return the ordinary bounded unavailable-tool outcome
AND SHALL NOT enter an approval state or dispatch review work.

**Rationale:** The absence of execution and lifecycle authority structurally prevents accidental token spend and avoids a second review workflow alongside ordinary pull-request review.

---

### REQ-CR-017: Recover Persisted Pending Approval Without Success

WHEN the system encounters persisted state awaiting commission-review approval
THE SYSTEM SHALL persist the carried assistant tool-use message
AND SHALL pair it with exactly one generic unavailable-capability error tool result for the same tool-use identifier
AND SHALL move the conversation to an interactive non-success state
AND SHALL NOT dispatch an LLM or tool execution.

THE migration SHALL be idempotent so that retry cannot duplicate transcript messages or dispatch work.

THE SYSTEM SHALL represent the migrated unavailable result as a typed settled tool call
AND SHALL NOT auto-continue or dispatch a provider while that result remains the transcript tail.

WHEN later transcript content follows the settled result
THE SYSTEM SHALL NOT use the earlier settlement to suppress recovery of the later content.

**Rationale:** An unavailable approval endpoint must not leave a conversation stuck. Preserving the request and recording an explicit error keeps the transcript structurally valid without fabricating review findings or success.

---

### REQ-CR-018: Preserve Historical Transcript Content Without Specialized Authority

WHEN a transcript contains a historical `commission_review` tool-use or tool-result block
THE SYSTEM SHALL preserve the stored message content
AND SHALL render it through the generic read-only historical tool surface
AND SHALL NOT provide specialized execution, approval, result parsing, navigation, or viewer authority.

WHEN a historical Close attempt captured the unsupported approval state
THE SYSTEM SHALL preserve that snapshot through a typed historical metadata sink
AND SHALL NOT make the captured value valid as live conversation state.

THE SYSTEM SHALL NOT guarantee that specialized viewer URLs, approval endpoints, older binaries, or downgrade paths are usable.

**Rationale:** Transcript content is user history, while specialized writable lifecycle and viewer contracts are product authority. Preserving the former does not justify retaining the latter.

## Deprecated Requirements

REQ-CR-001 through REQ-CR-015 are deprecated by ADR-038. They described the retired execution, approval, target inference, review collection, cancellation, and specialized result contracts and therefore impose no active product obligation.
