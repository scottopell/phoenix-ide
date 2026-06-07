# Phoenix Chains

## User Story

As a Phoenix user, I run long streams of related work as continuation
chains — for example, an "auth refactor" stream where conv #41 was
"continued in new conversation" into #42, which was continued into #44.
Days or weeks later I want to recall something specific from that chain
("What were the top optimizations we applied?") without:

- Continuing #44 just to ask, which pollutes ongoing work and spends
  context on retrieval rather than progress
- Starting a fresh conversation and re-explaining the entire scope
  before I can ask my actual question

I want to think of the chain as a unit — give it a recognizable name
("auth refactor"), find it nested in my sidebar, navigate to it as a
place, and ask it questions whose answers see all of the work that's
happened across its members.

## Why the User Cares

- **Recall without re-explanation** saves tokens and cognitive cost. A
  chain that contains weeks of work would otherwise require copying
  context manually or paying to replay it.
- **A named, navigable chain** is easier to find, share across browser
  tabs, and return to than a list of opaque conversation ids. The name
  is the hook the user remembers.
- **Asking a recall question without continuing** keeps work
  conversations focused on work, not retrieval. The user does not have
  to choose between "ask cheaply but pollute" and "ask cleanly but pay
  to re-explain."

## Transparency Contract

The user must be able to confidently answer:

1. Which conversations are in this chain, and in what order?
2. What work does this chain represent — which worktree, branch, task,
   and pull request is it driving, and what is that PR's state?
3. What questions have I already asked this chain, and what answers
   did I get?

## Requirements

### REQ-CHN-001: Recall Past Work Without Re-Explaining Context

WHEN a user wants to recall information from a continuation chain
THE SYSTEM SHALL provide an interaction surface that returns an answer
derived from the content of every member of that chain
AND SHALL NOT require the user to extend any of those conversations or
to re-supply their content as input

**Rationale:** Headline benefit. Without it the user pays full token
cost twice — once to do the work originally, and again to retrieve
from it. Including every member of the chain prevents partial-recall
failure modes: a "where did we leave off?" answer that ignores the
latest conversation in the chain would be misleading.

---

### REQ-CHN-002: Continuation Chains Surface as First-Class Entities

WHEN two or more conversations share a linear handoff lineage through
`continued_in_conv_id` (one was created via "continue in new conversation"
from another, or a managed Explore task approval was handed off to a fresh
Work conversation)
THE SYSTEM SHALL present them as a grouped chain in conversation
navigation surfaces, identifiable by the chain's root conversation as
its identity

THE SYSTEM SHALL render chain members visually nested under a
collapsible chain header in the sidebar, in chain order (root → latest)

WHEN a conversation has not been continued and was not itself a
continuation
THE SYSTEM SHALL render it as a standalone (non-chain) navigation entry

**Rationale:** Chain membership emerges automatically from how the
user already structures work via continuations — no manual grouping
action required. Visual nesting in the sidebar makes the chain a
tangible thing the user can perceive without ceremony. Keeping
standalone conversations ungrouped avoids visually inflating every
conversation into a degenerate one-member chain.

---

### REQ-CHN-003: Chain Page as a Navigable Place

WHEN the user activates a chain header in the sidebar (or otherwise
navigates to a chain)
THE SYSTEM SHALL navigate to a chain page that lists the member
conversations in chain order and provides an entry point for asking
the chain questions

THE SYSTEM SHALL support standard browser navigation (back button,
deep linking, refresh) to and from the chain page

**Rationale:** A named chain that you can see but cannot navigate to
is a label, not a place. Deep-linkable URLs and browser-native
navigation are the foundational guarantees of a place; absent them the
chain has no stable destination for revisiting Q&A history or
sharing across browser tabs.

---

### REQ-CHN-004: Ask the Chain, Get a Streamed Answer

WHEN the user submits a question on a chain page
THE SYSTEM SHALL produce an answer derived from the chain's
conversation content, streamed token-by-token to the user as it is
generated

WHILE an answer is being prepared but no tokens have arrived
THE SYSTEM SHALL display a progress indication that signals the request
is in flight

WHILE tokens are arriving
THE SYSTEM SHALL render them incrementally rather than waiting for the
full answer

**Rationale:** Q&A is the headline interaction; streaming and
loading-state quality are explicit user requirements. A half-rendered
loading state would undermine confidence even when the answer itself
is good.

---

### REQ-CHN-005: Q&A History Persists Per Chain

WHEN a user has previously asked questions on a chain
THE SYSTEM SHALL display the prior questions and answers when the
chain page is reopened

THE SYSTEM SHALL render the Q&A panel as a vertical list of pair cards
where each pair card displays an explicit `Q:` row and `A:` row. There
SHALL always be exactly one **active pair card** at the top of the
panel whose `Q:` row is an empty, autofocused textarea and whose `A:`
row is a "waiting for question" placeholder. Persisted and currently
streaming pairs SHALL render below the active card in reverse
chronological order, with the most recent pair immediately below the
active card.

> **Superseded by REQ-CHN-009.** Snapshot staleness was a property of
> the summaries-bundling Q&A: an answer was computed against a fixed
> snapshot, so a later-advanced chain could make it stale. Under
> retrieval-backed Q&A every question runs against the live index, so an
> answer is never "stale relative to a snapshot" — there is no snapshot.
> The staleness indicator and the per-answer snapshot counters it relied
> on are removed. The clause below is retained for traceability only.

WHEN a stored Q&A answer was generated against an earlier snapshot of
the chain (members or per-member message counts have changed since the
answer was produced)
THE SYSTEM SHALL visually indicate the answer's snapshot staleness so
the user can tell at a glance whether re-asking would likely yield a
materially different answer

WHEN a stored Q&A is in an incomplete or failed state (the stream
ended without producing a complete answer)
THE SYSTEM SHALL render the question and a clear failure indicator so
the user sees their question wasn't lost and can re-ask if desired

**Rationale:** Users return to chains. Without persistence they lose
answers they paid to generate and have no record of what they have
already asked. Pair cards reinforce REQ-CHN-006's independence
guarantee structurally — each Q&A is a self-contained object, and the
active card is visibly the same shape as past pairs (just unfilled), so
the user understands their next question creates a new pair rather than
continuing a thread. Reverse-chronological ordering keeps the freshest
context next to the active card without requiring the user to scroll.
Snapshot staleness prevents acting on stale recall — "where did we
leave off?" captured before the latest conversation was added would
mislead without this signal. Surfacing failed/incomplete Q&A preserves
the user's question text rather than losing it on stream failure.

---

### REQ-CHN-006: Consistent Quality As Q&A Accumulates

WHILE a user is asking questions on a chain page
THE SYSTEM SHALL produce answers whose quality, latency, and content do
not materially degrade as more questions and answers accumulate in
that chain's Q&A history

**Rationale:** Each question is answered against the canonical chain
content, not against the model's own prior answers. This prevents
drift (early misunderstandings compounding into later answers) and
bounds cost as Q&A history grows. The user-visible property is that
the tenth question feels as fast and accurate as the first.

**Implication:** v1 Q&A invocations are intentionally disjoint — the
model does not see prior questions or answers from the same chain. A
follow-up like "tell me more about #2" will not work unless the user
restates the prior context in the new question. The Q&A panel
communicates this independence visually (each Q&A is a self-contained
card with no chat-style ligatures). See the non-requirements list for
the v1.5 path that addresses this without breaking REQ-CHN-006.

---

### REQ-CHN-007: Chain Has a User-Editable Name

WHEN a chain is first surfaced in the UI
THE SYSTEM SHALL display a name for it derived from the chain's root
conversation title

WHEN the user invokes a name-edit action on the chain page header
THE SYSTEM SHALL allow inline editing of the chain name and persist
the new value when the user commits (Enter, blur, or explicit confirm)

THE SYSTEM SHALL display the user-set name (when present) consistently
in every place the chain is identified — sidebar header, chain page
header, and any other UI surface that names the chain

**Rationale:** The chain is going to be a recognizable visual entity in
the sidebar. Names are the hook users remember and search for. A
user-set "auth refactor" is more findable than the auto-derived title
of conv #41. Editing inline (rather than in a settings modal) keeps
the chain feeling like a lightweight entity rather than a heavyweight
configurable object.

---

### REQ-CHN-008: Chain Page Surfaces the Work Scope

A chain's members are linked by continuation, but the thing they are
all *working on* — the worktree, the branch, the task, the pull request
— is shared across the chain and is the chain's real subject. Managed
and Branch work preserve their worktree across context-exhaustion
continuations and across the Explore→Work handoff, so a chain's members
overwhelmingly share one work scope (`crate::work_scope::WorkScope`),
even though chain membership (continuation lineage) and work scope
(resource ownership) are distinct concepts that can in principle
diverge.

WHEN the chain page is displayed
THE SYSTEM SHALL surface the chain's work scope above the member list:
the worktree path, the branch and base branch, the task (id and title)
when the chain is doing Managed work, and the associated pull request
when one exists — its `display_state` (open / draft / merged / closed),
checks, and feedback-freshness signal as already tracked per work scope

WHEN the chain spans more than one work scope (a member diverged onto a
different worktree, or a member is Direct/conversation-scoped)
THE SYSTEM SHALL represent that honestly rather than collapsing the
chain to a single arbitrary scope — the panel reflects the actual set
of scopes the chain touches

WHEN the chain has no work scope beyond conversation identity (e.g. a
chain of Direct conversations with no worktree)
THE SYSTEM SHALL indicate the absence of a managed work scope rather
than showing empty worktree/branch/PR fields

**Rationale:** The member list answers "what conversations happened";
it does not answer "what is this chain *for*." The worktree / branch /
task / PR is the through-line that makes the chain a unit of work rather
than a list of transcripts, and it is information Phoenix already tracks
per work scope (`work_scope_pr_associations`, the `ConvMode` git
metadata) but the chain page omits. Surfacing it satisfies the
Transparency Contract's "what work does this chain represent" question.
Keeping the chain concept while adding this panel — rather than
renaming the chain to a "work scope" — is deliberate: continuation
lineage and resource ownership are not the same thing, and the
near-1:1 correspondence is a strong default, not an invariant to
hard-code.

---

### REQ-CHN-009: Chain Q&A Is a Read-Only Agentic Loop

WHEN the user asks a chain a question
THE SYSTEM SHALL answer it by running a read-only agent that is given
tools to (a) search the chain's conversation content by relevance and
(b) read the full content of any chain member, and that iterates —
searching, reading, and reasoning — until it can answer, then streams
the answer

THE agent's tools SHALL be **scope-bound to the chain's members**: the
search tool retrieves only across the chain's member conversations
(`specs/conversation-retrieval/` REQ-RET-001 with
`Conversations(member_ids)`), and the read tool can fetch only the
content of conversations in that member set. The model SHALL NOT be able
to widen its own scope to conversations outside the chain.

THE agent SHALL be **read-only**: it is given no tool that mutates
state (no bash, no patch, no worktree access). Its only side effect is
producing the streamed answer.

THE SYSTEM SHALL NOT restrict non-leaf members to their trailing
continuation summary; the agent can read any member's actual message
content on demand.

THE agent SHALL run against the live index and live message content at
query time, so an answer reflects the chain's current state by
construction (this is what supersedes REQ-CHN-005's snapshot-staleness
machinery).

THE agent SHALL NOT see prior Q&A questions or answers from the same
chain (REQ-CHN-006 holds): each question is a fresh agent run that may
iterate internally but carries no cross-question memory.

**Rationale:** A one-shot bundle — whether of summaries or of a single
retrieval pass — caps the answer at whatever context was guessed up
front. The capability the user actually wants is "go dig through the
whole conversation": let the model search, decide what looks relevant,
read it in full, and search again if the first pass missed. That is an
agent loop, and it is the version that genuinely has access to the
entire conversation rather than a pre-flattened slice of it. Binding
the tools to the chain's member set by construction (the host fixes the
scope; the model only supplies the query) keeps the agent from
wandering outside the chain while still giving it full depth within it.
Read-only because Q&A is recall, not work. Reusing the product-wide
retrieval primitive as the search tool means the same agent, pointed at
the `Global` scope, becomes the future application-wide Q&A — chain Q&A
and global Q&A differ only in the scope the host binds into the tools.
Keeping each question a fresh run preserves REQ-CHN-006's
no-cross-question-drift guarantee; the cost/latency now varies with
question difficulty rather than chain size, which is the intended
trade — a pointed question stays cheap, a deep one is allowed to work
for it.

---

## Non-Requirements (explicit out-of-scope for v1)

- **Kickstart action / offshoots / tree-shaped chains.** Deferred
  decision: the worktree-ownership invariant introduced when peer
  conversations would share or fork a worktree is unspecified upstream
  and warrants its own spec before kickstart can ship coherently. v1
  chains are linear (continuation only).
- **Resume as a first-class action.** Sidebar nesting already shows
  the latest member at the bottom of the chain block, and the chain
  page emphasizes the latest-active member visually. A separate
  Resume button is redundant. The user clicks the latest member's
  card to resume.
- **Manual chain membership editing.** Adding or removing arbitrary
  conversations from a chain. Membership stays derived from
  `continued_in_conv_id`; supported edges are context continuation and
  approved-task fresh Work handoff.
- **Q&A editing or deletion.** Q&A history is append-only.
- **Follow-up Q&A with prior-Q&A model context.** REQ-CHN-006 keeps
  invocations stateless; the model never sees prior Q&A from the
  same chain. Named v1.5 path: a "reply" affordance on each prior
  Q&A pre-fills the input with a quoted snippet so the user's
  question becomes self-contained, preserving the stateless contract
  that protects REQ-CHN-006.
- **Cross-chain linking or comparison.** No requirement defines it.
- **Project-level summary or steering doc.** A separate concept,
  explicitly deferred.

## Future Direction (named, not v1)

- **Retrieval-backed Q&A architecture (now specified).** Chain Q&A
  assembling context by similarity retrieval rather than by bundling
  per-member summaries is specified by REQ-CHN-009 and the product-wide
  primitive in `specs/conversation-retrieval/`. The MVP backend is
  lexical (FTS5/BM25); a vector/hybrid backend behind the same seam
  remains future work, as does the application-wide Q&A surface that the
  primitive's `Global` scope is built to serve.
- **Kickstart (deferred from this spec).** "Spawn a related
  conversation in a different direction" has real user value but
  requires resolving the worktree-ownership invariant for peer
  conversations first (a `specs/projects/` concern, not a chains
  concern). **Trigger to pivot:** a worktree-peer-ownership spec
  exists and defines coherent semantics for two long-lived
  conversations sharing or forking a worktree.
