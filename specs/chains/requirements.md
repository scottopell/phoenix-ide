# Phoenix Chains

## User Story

As a Phoenix user, I sometimes continue one workstream across multiple
stored conversation rows. Later, I reopen the conversation from the
normal conversation list and want one place where I can:

- read the full transcript history in the order the work actually
  happened
- understand what work this conversation is attached to now
- ask a recall question across the whole workstream without reopening
  old rows or re-explaining the context

I do not want Phoenix to make me manage a separate product object for
that lineage. The durable root conversation is the product identity, and
continuation-linked rows are implementation topology Phoenix uses to
preserve the history of that one product conversation.

## Why the User Cares

- **One conversation identity is easier to find and trust.** The user
  remembers a conversation title, not a separate chain object with its
  own lifecycle.
- **Recall without re-explanation** saves tokens and cognitive cost.
  Weeks of prior work should remain explorable without replaying the
  whole context manually.
- **Work stays on the normal page.** The user should not have to decide
  whether to visit a transcript page or a separate lineage page to keep
  working.

## Transparency Contract

The user must be able to confidently answer:

1. Which stored conversation rows belong to this one product
   conversation, and in what order?
2. What work does this conversation represent right now — which
   worktree, branch, task, and pull request is it attached to?
3. What recall questions have I already asked from this conversation,
   and what answers did I get?
4. Does a stored answer predate later transcript activity and therefore
   deserve re-checking?

## Requirements

### REQ-CHN-001: Recall Past Work Without Re-Explaining Context

WHEN a user wants to recall information from a product conversation that
spans continuation-linked rows
THE SYSTEM SHALL provide an interaction surface that returns an answer
derived from the content of every member row in that lineage
AND SHALL NOT require the user to extend any of those rows or to
re-supply their content as input

**Rationale:** The user already paid to create this work history.
Retrieval should use the whole lineage as one conversation-level context
rather than forcing the user to replay it manually.

---

### REQ-CHN-002: Continuation Lineage Is Navigation Topology, Not a Separate Product Entity

WHEN two or more stored conversation rows share a linear handoff lineage
through `continued_in_conv_id`
THE SYSTEM SHALL treat them as one product conversation whose durable
identity is the root conversation row

THE SYSTEM SHALL render that product conversation as a single entry in
conversation navigation surfaces keyed by the durable root conversation
rather than as nested member rows under a separate chain header

WHEN a conversation has no continuation lineage beyond its durable root
row
THE SYSTEM SHALL still render it as one product conversation entry using
that same root-keyed identity model

**Rationale:** Continuation rows preserve transcript topology and latest-row
authority, but the user-facing product object remains one conversation.
Showing a separate chain container with nested members would create a
second product identity for the same work.

---

### REQ-CHN-003: The Normal Conversation Surface Hosts Lineage History

WHEN the user opens a product conversation whose history spans
continuation-linked rows
THE SYSTEM SHALL navigate to the normal conversation surface for the
root conversation
AND that surface SHALL render the transcript history in lineage order
and host any lineage-wide recall interaction on that same page

THE SYSTEM SHALL support standard browser navigation (back button, deep
linking, refresh) to and from that normal conversation surface

THE SYSTEM SHALL NOT require a dedicated chain route, chain page, or
chain-only header to access lineage history or lineage Q&A

**Rationale:** The user needs one stable place for the conversation, but
that place is the ordinary conversation page. The lineage is part of the
conversation's history, not a second destination.

---

### REQ-CHN-004: Lineage Q&A Streams on the Normal Conversation Surface

WHEN the user submits a lineage recall question on the normal
conversation surface
THE SYSTEM SHALL produce an answer derived from the product
conversation's lineage content, streamed token-by-token to the user as
it is generated

WHILE an answer is being prepared but no tokens have arrived
THE SYSTEM SHALL display a progress indication that signals the request
is in flight

WHILE tokens are arriving
THE SYSTEM SHALL render them incrementally rather than waiting for the
full answer

**Rationale:** Streaming is part of the user-visible quality bar for
recall. The page hosting the transcript should also host the live answer
experience.

---

### REQ-CHN-005: Q&A History Persists With the Product Conversation

WHEN a user has previously asked lineage-wide recall questions from a
product conversation
THE SYSTEM SHALL display the prior questions and answers when that same
conversation surface is reopened

THE persisted Q&A history MAY remain attached to the durable root
conversation row so long as the user experiences it as history belonging
to that one product conversation rather than to a separate chain object

WHEN a stored Q&A answer was produced before the lineage grew or later
member rows accumulated additional messages
THE SYSTEM SHALL show an age-of-answer freshness indicator on that
stored answer so the user can tell it may predate later conversation
activity

WHEN a stored Q&A is in an incomplete or failed state
THE SYSTEM SHALL render the question and a clear failure indicator so the
user sees their question was not lost and can re-ask if desired

**Rationale:** Users return to prior recall answers. Persisting that
history on the root is acceptable if it preserves one conversation-level
experience and does not invent a separate Q&A lifecycle.

---

### REQ-CHN-006: Independent Q&A Quality As History Accumulates

WHILE a user is asking lineage-wide recall questions from the normal
conversation surface
THE SYSTEM SHALL produce answers whose quality, latency, and content do
not materially degrade as more Q&A history accumulates for that product
conversation

**Rationale:** Each recall question should stand on the conversation's
canonical transcript history, not on accumulated prior answers. The
user-visible result is that later questions remain as trustworthy and
responsive as early ones.

**Implication:** Q&A invocations are intentionally disjoint. The model
does not see prior questions or answers unless the user restates them in
a new question.

---

### REQ-CHN-007: Conversation Title Belongs to the Root Product Conversation

WHEN a continuation-linked lineage is surfaced to the user
THE SYSTEM SHALL identify it using the root product conversation's title
or rename value

THE SYSTEM SHALL treat conversation naming as part of the normal
conversation title/rename behavior for that root product conversation
rather than as a separate chain-specific naming action or lifecycle

THE SYSTEM SHALL apply the chosen title consistently anywhere this one
product conversation is named

**Rationale:** The user names a conversation, not a second chain object.
A separate chain-name affordance would duplicate the ordinary
conversation title concept and risk divergence.

---

### REQ-CHN-008: The Normal Conversation Surface Shows Work Identity for the Live Attached Scope

WHEN the normal conversation surface is displayed for a product
conversation with continuation-linked history
THE SYSTEM SHALL surface, for the attached live work scope, its work
identity — worktree path, current branch, base/default branch context,
and any associated task context — and its pull-request health when an
associated PR exists: `display_state` (open / draft / merged / closed),
checks, and feedback-freshness signal

THE SYSTEM SHALL resolve this work identity from the attached
`WorkScope` / latest live conversation-row authority and present it on
the normal conversation surface rather than requiring a separate chain
surface

WHEN the conversation has no Git-backed work scope
THE SYSTEM SHALL indicate the absence of a Git-backed work scope rather
than rendering empty worktree/branch/PR fields

**Rationale:** The user needs to know what work this conversation is
attached to now. That identity belongs beside the transcript on the
ordinary page, even though the underlying authority resolves through the
latest live row.

---

### REQ-CHN-009: Lineage Q&A Is a Read-Only Agentic Loop Scoped to One Product Conversation

WHEN the user asks a lineage-wide recall question
THE SYSTEM SHALL answer it by running a read-only agent that is given
tools to (a) search the conversation content by relevance and (b) read
the full content of any member row in that conversation's continuation
lineage, and that iterates — searching, reading, and reasoning — until
it can answer, then streams the answer

THE agent's tools SHALL be scope-bound to that one product conversation:
the search tool retrieves only across that conversation's member rows,
and the read tool can fetch only the content of rows in that lineage

THE host SHALL bind the durable root conversation identity and resolve
the member-row set live per tool call rather than freezing it at run
start

THE agent SHALL be read-only: it is given no tool that mutates state.
Its only side effect is producing the streamed answer.

THE SYSTEM SHALL NOT restrict non-leaf member rows to continuation
summaries alone; the agent can read their actual message content on
demand

THE read tool SHALL return the full content of the messages it returns,
including tool-result bodies, build logs, and sub-agent output, subject
to bounded paging where needed

WHEN the retrieval index has not caught up to the conversation's
messages
THE SYSTEM SHALL NOT present a Q&A answer as if the full conversation
had been searched authoritatively; it SHALL either wait for coverage to
catch up or surface that recall coverage was partial

THE agent SHALL NOT see prior Q&A questions or answers from the same
conversation unless the user includes that context in the new question

**Rationale:** The user wants Phoenix to dig through the whole
conversation lineage, not through a lossy summary bundle. Binding the
agent to one root-keyed product conversation preserves that scope
without inventing a second product entity.

---

### REQ-CHN-010: No Separate Chain-Specific Lifecycle or Management Actions

WHEN a product conversation spans continuation-linked rows
THE SYSTEM SHALL NOT introduce separate chain-specific rename,
regenerate-name, archive, unarchive, delete, or dedicated management
actions that duplicate ordinary conversation actions for the same root
product conversation

THE SYSTEM SHALL treat continuation lineage as transcript/retrieval
scope within the product conversation rather than as a separately named,
archivable, or deletable object

**Rationale:** Separate management actions would imply a second
user-facing object with its own lifecycle. One root-keyed
ProductConversation already carries the transcript history and recall
scope for that work.

---

## Non-Requirements

- **Tree-shaped continuation groups.** This spec covers linear
  `continued_in_conv_id` lineage only.
- **Manual lineage membership editing.** Membership remains derived from
  continuation topology rather than hand-curated by the user.
- **Separate chain page, route, or sidebar container.** The conversation
  page is the surface; dedicated chain navigation is explicitly not part
  of the product model.
- **Separate chain rename or regenerate-name flow.** Conversation title
  behavior on the root product conversation is sufficient.
- **Separate chain archive or delete lifecycle.** Ordinary conversation
  lifecycle rules apply; there is no second chain lifecycle.
- **Follow-up Q&A with prior-answer model context.** Recall questions are
  intentionally independent.
- **Cross-conversation comparison outside one root-keyed product
  conversation.** This spec scopes recall to one product conversation's
  continuation lineage.
