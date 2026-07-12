# Phoenix Coordinator

## User Story

As a Phoenix user, I often have several streams of work active across
projects, continuation chains, and standalone conversations. When I return
to Phoenix, I want one place that answers "what active work exists?" without
asking an LLM to infer it from history. I also want a durable Coordinator
conversation where I can ask for cross-conversation orientation, planning, and
handoff help while still seeing the compact fleet snapshot beside it.

The Coordinator is not ambient memory for ordinary coding conversations. It is
a deliberate Phoenix-wide surface the user opens when they want global
orientation or synthesis.

## Why the User Cares

- **Orientation should be deterministic.** The user should not spend model
  tokens or trust an inference step just to see active work.
- **Long-running work should not fragment identity.** A continuation chain
  represents one work item even though it spans multiple conversations.
- **Synthesis should reuse the normal conversation experience.** The user
  should get Phoenix's standard transcript, composer, continuation, and
  persistence behavior rather than a parallel approximation.
- **Global access should be deliberate and bounded.** Normal coding agents stay
  focused on their scoped work; Phoenix-wide read tools are reserved for the
  Coordinator.

## Transparency Contract

The user must be able to answer:

1. Which project owns an open work item?
2. Is the item a continuation chain or a standalone conversation?
3. Which conversation is the current/latest source of truth for the item?
4. Why does the item appear in the fleet list?
5. Which source conversations or messages support a Coordinator answer?

## Requirements

### REQ-GR-001: View Active Work Without Model Inference

WHEN a user opens the Coordinator surface
THE SYSTEM SHALL present a deterministic fleet view of active Phoenix work
without requiring an LLM request

THE view SHALL group visible work by project

THE view SHALL include work backed by continuation chains and standalone
conversations

---

### REQ-GR-002: Collapse Continuation Chains Into One Work Item

WHEN conversations form a linear continuation chain
THE SYSTEM SHALL represent the chain as one fleet item identified by its
chain root

THE SYSTEM SHALL expose the current/latest conversation for the chain

THE SYSTEM SHALL NOT show non-leaf chain members as separate fleet items when
they are already represented by the chain item

WHEN historical chain members are archived
THE SYSTEM SHALL preserve the original chain root as the work-item identity if
the latest conversation still has positive open-work evidence

---

### REQ-GR-003: Explain Why Work Appears

WHEN a fleet item is visible
THE SYSTEM SHALL show deterministic signals that explain its inclusion or
priority

THE signals SHALL cover the item source, recent activity, work or branch mode,
active or attention-needing runtime state, task status when available, and
multi-member chain membership when applicable

THE SYSTEM SHALL include a Work-mode item only when its task status is
`in-progress`, `ready`, or `blocked`, or when its runtime state is active,
recovery-like, or attention-needing

THE SYSTEM SHALL include an otherwise-idle Direct, Explore, or Branch item only
when it was updated within 14 days

THE SYSTEM SHALL suppress completed, failed, handed-off, terminal, archived,
and non-user-initiated current conversations

IF a Work-mode task status is unavailable
THE SYSTEM SHALL NOT treat recency alone as evidence that the work remains open

---

### REQ-GR-004: Surface Work Identity Metadata

WHEN metadata is available for a fleet item
THE SYSTEM SHALL show the mode, task id, task title, task status, branch name,
base branch, worktree path, current conversation, root conversation, and last
update time

THE SYSTEM SHALL tolerate missing metadata without fabricating substitute
values

---

### REQ-GR-005: Provide Stable References and App-Local Links

WHEN a fleet item, chain, conversation, or source message is displayed as a
source
THE SYSTEM SHALL provide an app-local navigation target or stable reference
handle that can be copied or cited

THE reference syntax SHALL distinguish chains, conversations, and open work
items

WHEN an open work item later closes or becomes archived
THE SYSTEM SHALL continue to resolve its previously issued work-item reference
to the durable source identity and SHALL report its current open, closed, or
archived status

THE navigation targets SHALL be app-relative so deployment hostnames and
browser gateways do not determine reference validity

---

### REQ-GR-006: Provide One Durable Coordinator Identity

WHEN a user opens the Coordinator surface
THE SYSTEM SHALL resolve it to exactly one durable Coordinator conversation
identity

THE SYSTEM SHALL create that Coordinator conversation on demand when it does
not yet exist

THE SYSTEM SHALL NOT present the Coordinator as ordinary project coding work or
as a user-created member of the fleet list

---

### REQ-GR-007: Restrict Phoenix-wide Tools to the Coordinator

WHILE a normal coding conversation is running
THE SYSTEM SHALL NOT provide unrestricted Phoenix-wide history search or read
tools by default

WHILE the Coordinator is answering a user question
THE SYSTEM MAY provide read-only host-bound tools for global message search,
paged conversation reads, deterministic fleet reads, and reference resolution

THE SYSTEM SHALL NOT provide filesystem mutation, task approval, task drafting,
or workspace-management tools to the Coordinator

---

### REQ-GR-008: Answer With Source Citations

WHEN the Coordinator answers a question using conversation history
THE SYSTEM SHALL instruct the answering agent to cite source conversations or
messages using app-local links or stable reference handles

THE SYSTEM SHALL expose enough source metadata through read-only tools for the
agent to cite the conversation id, message id when available, role, timestamp,
and excerpt or read content that supports the answer

---

### REQ-GR-009: Resolve Copied References

WHEN a user or the Coordinator provides a supported reference
THE SYSTEM SHALL resolve it to its target kind, target id, app-local navigation
target when available, title when available, and a concise summary

IF the reference has unsupported syntax
THE SYSTEM SHALL return a clear error instead of guessing silently

---

### REQ-GR-010: Keep the Fleet Snapshot Visible on the Coordinator Surface

WHEN a user opens `/global`
THE SYSTEM SHALL keep the compact fleet snapshot visible within the Coordinator
surface instead of replacing the page with a bare conversation redirect

THE SYSTEM SHALL allow the user to expand a fleet row to inspect detailed
signals and metadata without leaving the Coordinator surface
