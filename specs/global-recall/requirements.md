# Phoenix Global Recall

## User Story

As a Phoenix user, I often have several streams of work active across
projects, continuation chains, and standalone conversations. When I return
to Phoenix, I want one place that answers "what active work exists?" without
asking an LLM to infer it from history. I also want a separate read-only
analysis space where I can ask strategic questions across conversation
history, produce handoff reports, and cite the source conversations that
support the answer.

Global Recall is not ambient memory for ordinary coding conversations. It is
a deliberate global surface the user opens when they want cross-conversation
orientation or synthesis.

## Why the User Cares

- **Orientation should be deterministic.** The user should not spend model
  tokens or trust an inference step just to see active work.
- **Long-running work should not fragment recall.** A continuation chain
  represents one work item even though it spans multiple conversations.
- **Synthesis needs provenance.** Cross-conversation answers are only useful
  when the user can inspect the source conversations and messages.
- **Global access should be deliberate.** Normal coding agents stay focused on
  their scoped work; global-history tools are reserved for the Global Recall
  surface.

## Transparency Contract

The user must be able to answer:

1. Which project owns an open work item?
2. Is the item a continuation chain or a standalone conversation?
3. Which conversation is the current/latest source of truth for the item?
4. Why does the item appear in the open-work list?
5. Which source conversations or messages support a recall answer?

## Requirements

### REQ-GR-001: View Active Work Without Model Inference

WHEN a user opens the Global Recall surface
THE SYSTEM SHALL present a deterministic Global Open Work view of active
Phoenix work without requiring an LLM request

THE view SHALL group visible work by project

THE view SHALL include work backed by continuation chains and standalone
conversations

**Rationale:** The user's first need is orientation. Deterministic projection
makes the list explainable, cheap, and repeatable; model synthesis belongs in
separate recall sessions, not in the basic "what is open?" answer.

---

### REQ-GR-002: Collapse Continuation Chains Into One Work Item

WHEN conversations form a linear continuation chain
THE SYSTEM SHALL represent the chain as one open work item identified by its
chain root

THE SYSTEM SHALL expose the current/latest conversation for the chain

THE SYSTEM SHALL NOT show non-leaf chain members as separate open work items
when they are already represented by the chain item

WHEN historical chain members are archived
THE SYSTEM SHALL preserve the original chain root as the work-item identity if
the latest conversation still has positive open-work evidence

**Rationale:** A continuation chain is the user's unit of work. Listing every
member separately creates clutter and makes old chain members look like
independent active work.

---

### REQ-GR-003: Explain Why Work Appears

WHEN an open work item is visible
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

**Rationale:** The user needs confidence that the global list is not magic.
Explicit signals turn the projection into an auditable triage view.

---

### REQ-GR-004: Surface Work Identity Metadata

WHEN metadata is available for an open work item
THE SYSTEM SHALL show the mode, task id, task title, task status, branch name,
base branch, worktree path, current conversation, root conversation, and last
update time

THE SYSTEM SHALL tolerate missing metadata without fabricating substitute
values

**Rationale:** Open work is actionable only when the user can recognize what it
is connected to. Missing data should be visibly absent rather than guessed.

---

### REQ-GR-005: Provide Stable References and App-Local Links

WHEN a work item, chain, conversation, or source message is displayed as a
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

**Rationale:** Global Recall outputs need to be shareable inside Phoenix and
robust across deployments. App-local links and typed handles keep citations
portable within the product.

---

### REQ-GR-006: Create Saved Read-Only Recall Sessions

WHEN a user creates a Global Recall session
THE SYSTEM SHALL persist a separate saved analysis session owned by the Global
Recall surface

THE SYSTEM SHALL allow multiple Global Recall sessions to exist at the same
time

THE SYSTEM SHALL NOT present Global Recall sessions as ordinary project coding
work

**Rationale:** Strategic analysis often needs separate contexts: one handoff
report, one prioritization pass, one investigation. These sessions should not
pollute project work lists or coding-agent context.

---

### REQ-GR-007: Restrict Global Tools to Global Recall Sessions

WHILE a normal coding conversation is running
THE SYSTEM SHALL NOT provide unrestricted global-history search or read tools
by default

WHILE a Global Recall session is answering a user question
THE SYSTEM MAY provide read-only host-bound tools for global message search,
paged conversation reads, deterministic open-work reads, and reference
resolution

THE SYSTEM SHALL NOT provide filesystem mutation, task approval, task drafting,
or workspace-management tools to a Global Recall session

**Rationale:** Global history is powerful and broad. Keeping it off ordinary
coding agents prevents ambient-memory behavior, while read-only tools let the
deliberate global surface answer cross-conversation questions safely.

---

### REQ-GR-008: Answer With Source Citations

WHEN a Global Recall session answers a question using conversation history
THE SYSTEM SHALL instruct the answering agent to cite source conversations or
messages using app-local links or stable reference handles

THE SYSTEM SHALL expose enough source metadata through read-only tools for the
agent to cite the conversation id, message id when available, role, timestamp,
and excerpt or read content that supports the answer

**Rationale:** Cross-conversation synthesis is easy to over-trust. Citations let
the user verify claims, continue from the right conversation, or copy a handoff
with traceable evidence.

---

### REQ-GR-009: Resolve Copied References

WHEN a user or recall agent provides a supported Global Recall reference
THE SYSTEM SHALL resolve it to its target kind, target id, app-local navigation
target when available, title when available, and a concise summary

IF the reference has unsupported syntax
THE SYSTEM SHALL return a clear error instead of guessing silently

**Rationale:** Copied handles become useful only if the product can turn them
back into source material predictably. Explicit errors avoid confusing one
conversation, chain, or work item for another.
