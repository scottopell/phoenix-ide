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
2. What questions have I already asked this chain, and what answers
   did I get?
3. Was this answer generated against the current state of the chain,
   or has the chain advanced since then?

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

WHEN two or more conversations share a continuation lineage (one was
created via "continue in new conversation" from another)
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
  `continued_in_conv_id`.
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

- **Retrieval-backed Q&A architecture.** v1 bundles every chain
  member's continuation summary plus the leaf transcript (or an
  in-process leaf summary) into the model invocation. This scales
  linearly with chain size, regardless of how specific the question
  is. A retrieval architecture (per-message embeddings + similarity
  retrieval at query time) would scale with question specificity
  rather than chain size, structurally eliminate snapshot staleness
  (every query retrieves at current state), and pay off across
  Phoenix's whole product (any conversation could pull related
  context). **Trigger to pivot:** bundling cost becomes painful at
  observed chain sizes, or a product-level decision to introduce
  ambient memory across non-chain conversations as well.
- **Kickstart (deferred from this spec).** "Spawn a related
  conversation in a different direction" has real user value but
  requires resolving the worktree-ownership invariant for peer
  conversations first (a `specs/projects/` concern, not a chains
  concern). **Trigger to pivot:** a worktree-peer-ownership spec
  exists and defines coherent semantics for two long-lived
  conversations sharing or forking a worktree.
