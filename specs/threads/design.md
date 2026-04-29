# Phoenix Threads — Design

## Architecture Overview

Threads are a *derived navigation primitive* over two existing graph edges:
`conversations.continued_in_conv_id` (continuation chain edges) and
`conversations.seed_parent_id` (kickstart edges, REQ-SEED-003). A thread is
a tree shape: a continuation chain forms the **main line**, and any
conversation whose chain root has a seed pointer into the thread's main
line forms a **offshoot**. No new fields are added to the `conversations`
table; no `threads` table is introduced. Persistence is limited to a new
`thread_qa` table for Q&A history.

The Q&A surface is a single per-thread persistent UI history, with stateless
per-question invocations of a mid-tier model. Q&A scope is **all thread
members** — main line and offshoots alike — so questions like "where did
we leave off?" see the full picture of work the user has done under this
thread. Kickstart reuses the seeded-conversations mechanism
(`specs/seeded-conversations/`) — kickstart is a special case of seeding
where the draft prompt source is a Q&A answer and the new conversation
becomes an offshoot member of the source thread.

## Thread Identity and Membership (REQ-THR-002)

A thread is identified by its **root conversation ID**: a conversation
that is itself a chain root (no predecessor via `continued_in_conv_id`)
and has no `seed_parent_id` set. The thread's root is also the topmost
conversation in the tree of related work.

**Continuation chain shape.** Continuation edges (`continued_in_conv_id`)
form linear chains — each conversation has at most one successor, enforced
schema-side by the column being scalar and application-side by
`Database::continue_conversation`'s idempotent `AlreadyContinued` outcome
(`src/db.rs`). The main line of a thread is the linear continuation chain
rooted at the thread's root.

**Offshoot shape.** Kickstart edges (`seed_parent_id` set on the new
conversation, pointing to the source thread's root) admit zero-to-many
successors per source root: a single thread root can sprout multiple
offshoots. Each offshoot is itself a continuation chain — its offshoot root
is a conversation whose `seed_parent_id` points into a thread's main line,
and the offshoot's chain extends forward via continuation edges as usual.

**Membership rule.** A conversation `C` is a member of thread `T` iff
either:

1. `C`'s chain root equals `T` (i.e., `C` is on `T`'s main line), OR
2. `C`'s chain root has `seed_parent_id == T` (i.e., `C` is on an offshoot
   sub-chain rooted at a kickstart from `T`).

This is "follow the seed pointer exactly once" — offshoots of offshoots
(nested kickstarts where one kickstart's chain root has a `seed_parent_id`
pointing to another kickstart's chain) are deliberately not flattened
into the topmost thread; they belong to whichever thread their immediate
seed pointer lands on.

**Forward walk via recursive CTE.** The forward walk loads all members
of a thread for the thread page in two queries (or one combined query):

```sql
-- Step 1: main line (continuation chain forward from root)
WITH RECURSIVE chain(id, next_id, depth) AS (
    SELECT id, continued_in_conv_id, 0
    FROM conversations WHERE id = ?
    UNION ALL
    SELECT c.id, c.continued_in_conv_id, chain.depth + 1
    FROM conversations c JOIN chain ON c.id = chain.next_id
)
SELECT id, depth FROM chain ORDER BY depth;
```

```sql
-- Step 2: offshoot roots (conversations with seed_parent_id = thread root)
SELECT id FROM conversations WHERE seed_parent_id = ?;
```

For each offshoot root returned by step 2, the same forward CTE in step 1 is
executed against that offshoot root to load the offshoot's continuation chain.
For typical thread depths (single-digit offshoots, single-digit chain
length per offshoot) this is a small bounded number of CTE invocations.

**Backward walk** (any member → its thread root) follows continuation back
to a chain root, then if that chain root has a `seed_parent_id`, it
returns that seed parent as the thread root; otherwise it returns the
chain root itself.

A conversation is a thread member iff the thread it belongs to has
total membership (main line + offshoots) of size ≥ 2. Single isolated
conversations with no continuation successors and no kickstarts pointing
to them are not threads.

## Sidebar Grouping (REQ-THR-002)

Conversations in the sidebar are sorted `ORDER BY updated_at DESC`
(`Database::list_conversations`, `src/db.rs`). Members of a long-lived
thread are not consecutive in this sort — unrelated conversations from
in-between sit between them. Sidebar grouping therefore performs
**thread-block extraction**:

1. The sidebar query annotates each conversation with its thread root
   conv ID (the topmost root, computed by the backward walk described
   above) or `null` if it is not a thread member.
2. Conversations belonging to the same thread are extracted from the flat
   recency list into a single collapsible block.
3. Each thread block is positioned at the recency rank of its
   most-recent member (main line or offshoot — whichever has the latest
   `updated_at`), so a thread with any recent activity rides at the top.
4. Within the thread block, members are rendered hierarchically:
   - **Main line** members listed first in chain order (root → latest)
   - **Offshoots** listed below the main line. Each offshoot is rendered as
     a sub-tree (offshoot root + its continuation chain), indented under
     the thread block. Multiple offshoots are listed in the order their
     offshoot roots were created.
5. Standalone conversations remain interleaved by recency between thread
   blocks.

Members within the block are ordered by structural position (main line
then offshoots), independent of their individual `updated_at` values, so
the topology of the thread is legible at a glance.

Thread display name is the root conversation's title. Each thread block
defaults to expanded; expand/collapse state is not persisted across
navigations.

## Thread Page (REQ-THR-003, REQ-THR-005, REQ-THR-009)

Route: `/threads/:rootConvId`. The route is deep-linkable, supports browser
back/forward, and survives refresh.

**Page header.** The header surfaces the two header-level first-class
actions from the thread (the third, Kickstart, is per-answer and lives
inside the Q&A panel):

- **Resume** (REQ-THR-009) — a primary button labeled "Resume <member
  title>" where <member title> is the title of the thread's most
  recently active member (highest `updated_at` across main line and all
  offshoots). Clicking navigates to that member's conversation detail
  page in ready-to-work state (input focused, history loaded).
- **Snapshot indicator** — a passive header element showing the
  thread's current total member count and the timestamp of the most
  recently active member. This is the anchor against which Q&A
  snapshot-staleness (REQ-THR-005) is computed.

The Q&A input is always visible in the right column, so "Ask" is not
a header-level button — focus is one click away on the input itself.

**Layout (two-column):**

- **Left:** member conversations rendered hierarchically:
  - The **main line** at the top, as cards in chain order (root → latest).
    Each card shows title, position label (root / continuation / latest),
    date, and message count. The card for the most-recently-active member
    of the entire thread is visually emphasized (e.g., highlighted border
    or an "active" badge) so the Resume target is obvious before the
    user even reads the header button.
  - **Offshoots** below the main line. Each offshoot is a sub-tree: an
    offshoot root card (labeled "kickstarted from <Q&A excerpt>") followed
    by its own continuation chain cards indented underneath.
  Clicking any member card navigates to that conversation's detail page
  in the same ready-to-work state as the Resume action — REQ-THR-009 makes
  no distinction between Resume-via-header-button and Resume-via-card-click.
- **Right:** Q&A panel. Input box anchored at the bottom of the panel.
  Q&A history fills the area above the input, in chronological order
  (oldest at the top of the scroll region, most recent immediately above
  the input). Streaming answers render in place into the slot just above
  the input, flowing downward toward the input as tokens arrive — older
  Q&A above is not displaced. Each Q&A entry shows the question, the
  answer, and a Kickstart button.

  **Q&A entry independence (REQ-THR-006).** Each Q&A entry is rendered
  as a self-contained card — clear border, vertical gap from siblings,
  no visual ligatures between entries (no thread/reply lines, no avatar
  continuity, no "indenting" follow-ups). This is a deliberate visual
  signal that questions are answered independently against the thread's
  content rather than as a continuous conversation. The input box is
  always empty after submission; it does not preserve drafts across
  submissions and does not "thread" into the previous answer.

  **Q&A snapshot indicator (REQ-THR-005).** Each Q&A card displays a
  subtle inline tag indicating the thread state at answer time when it
  differs from current state — e.g., "answered when thread had 3
  conversations (now 5)" or "answered when offshoot #51 had 4 messages
  (now 9)". The tag is computed from a snapshot recorded on the
  `thread_qa` row at answer time (see Q&A Persistence) compared against
  the thread's current member set and per-member message counts.

## Q&A Backend (REQ-THR-001, REQ-THR-004, REQ-THR-006)

**Per-question model invocation receives:**

1. The thread's bundled context (defined below)
2. The user's current question
3. An instructional system prompt directing the model to answer from the
   provided context only and to indicate uncertainty when the context does
   not support a confident answer

The invocation does **not** receive prior Q&A history from the same thread.
This is what makes REQ-THR-006's user-visible property hold: each question
is answered against the canonical thread content, not against the model's
own prior answers.

**Context bundling.** The bundled context covers **all thread members** —
the main line and every offshoot sub-chain. Each member contributes one
context block, classified by whether it has been continued:

| Member kind | Context source |
|---|---|
| **Non-leaf** (continued into a successor; applies to non-final main-line members and non-final offshoot-chain members alike) | The trailing `MessageType::Continuation` message at the end of the member itself, persisted by `Effect::persist_continuation_message` during the `AwaitingContinuation → ContextExhausted` transition (`src/state_machine/transition.rs`). Its payload describes the work done in that member. |
| **Leaf** (not continued; the last member of the main line and the last member of each offshoot sub-chain are all leaves) | Transcript sent directly when the leaf has ≤ 20 messages and approximate token count ≤ 4000; otherwise an on-demand summary generated by the same mid-tier model in a pre-step before the main answer call. |

A thread with one main line of length `m` and `b` offshoots with chain
lengths `c₁, c₂, …, c_b` has `1 + b` leaves (one main-line leaf, one
per offshoot) and `(m - 1) + Σ(cᵢ - 1)` non-leaf members. Cost grows
linearly with member count, but each member contributes a bounded chunk.
The bundled context is labeled per-member with a structural tag
(e.g., `[main:#42]`, `[offshoot:#51]`, `[offshoot:#51→#52]`) so the model
can distinguish main-line work from offshoot work when the question
benefits from that distinction.

The leaf-summary thresholds are pinned to single shared values across
all Q&A invocations to avoid behavior drift between identical questions.

**Leaf-summary persistence.** When any leaf requires summarization, the
result is persisted to a new `thread_leaf_summary` table rather than held
in process memory:

| Column | Type | Notes |
|---|---|---|
| `leaf_conv_id` | TEXT NOT NULL | The leaf conversation; `REFERENCES conversations(id) ON DELETE CASCADE` |
| `message_count_at_summary` | INTEGER NOT NULL | The leaf's `message_count` at the moment this summary was generated |
| `summary` | TEXT NOT NULL | The summary content |
| `model` | TEXT NOT NULL | Model identifier used to generate this summary |
| `created_at` | DATETIME NOT NULL | UTC; set when the summary was generated |

Primary key: `(leaf_conv_id, message_count_at_summary)`.

**Lookup.** Before summarizing a leaf, the Q&A backend queries
`SELECT summary FROM thread_leaf_summary WHERE leaf_conv_id = ? AND
message_count_at_summary = ?` using the leaf's current `message_count`.
A hit serves the persisted summary directly with no model call. A miss
triggers summarization and INSERTs the new row.

**Staleness model.** The compound primary key encodes staleness
implicitly: a summary is fresh iff its `message_count_at_summary` matches
the leaf's current `message_count`. When the leaf grows by new messages,
the previous row remains as audit history (a record of what context the
model saw at past Q&A times) but no longer matches lookups; a new row is
generated and inserted on the next Q&A invocation that needs it. Old
rows are not GC'd in v1 — they are small, and retaining them gives a
useful audit trail for debugging Q&A answers in retrospect.

**Cascade.** When a leaf conversation is hard-deleted, its summaries are
removed via the foreign-key cascade.

**Restart behavior.** Because summaries are persisted, server restart
does not invalidate them — the first Q&A after restart picks up existing
fresh rows rather than regenerating them, as long as the leaf hasn't
grown in the interim.

**Model.** A mid-tier model balanced for cost and accuracy (Claude
Sonnet-class as of this writing). The model identifier is set at the
Q&A call site; there is no per-thread or per-user override. The
`thread_leaf_summary.model` column records which model produced each
persisted summary so future model upgrades can either reuse or
regenerate as appropriate.

**Streaming.** The Q&A response stream uses the existing SSE token-streaming
infrastructure (`specs/sse_wire/`). Streams are routed over a thread-scoped
channel keyed by `root_conv_id`, with each token event carrying the
per-question `thread_qa.id` as a request discriminator. This allows
multiple subscribers (e.g., the same thread page open in two tabs) to
demultiplex concurrent Q&As — a subscriber that submitted question A does
not render tokens from a sibling-tab's question B.

**Loading UX (REQ-THR-004).** Two visual states:

- **Pre-token** (request in flight, no tokens yet): a skeleton placeholder
  in the answer slot indicating the model is preparing
- **Streaming** (tokens arriving): incremental render token-by-token

## Q&A Persistence (REQ-THR-005)

New table `thread_qa`:

| Column | Type | Notes |
|---|---|---|
| `id` | TEXT PRIMARY KEY | UUID; doubles as the per-question request id for streaming demux |
| `root_conv_id` | TEXT NOT NULL | Thread identity; `REFERENCES conversations(id) ON DELETE CASCADE` |
| `question` | TEXT NOT NULL | User's submitted question |
| `answer` | TEXT NULL | Final assembled answer when the stream completes; NULL while in-flight or after stream failure/cancellation |
| `model` | TEXT NOT NULL | Model identifier used for the answer |
| `created_at` | DATETIME NOT NULL | UTC; set when the question is submitted |
| `completed_at` | DATETIME NULL | UTC; set when the answer stream completes successfully |
| `thread_snapshot` | TEXT NOT NULL | JSON-encoded snapshot of `[{conv_id, message_count}, …]` for every member of the thread at answer time; used to compute the staleness indicator (REQ-THR-005) on subsequent reads |

Index: `CREATE INDEX idx_thread_qa_root ON thread_qa(root_conv_id, created_at)`
so the per-thread lookup query is index-served.

**Snapshot computation.** When a question is submitted, the backend
walks the thread's members (forward CTE on the main line, plus
seed-pointer query for offshoot roots and their forward CTEs) and
records each member's id and current `message_count` into
`thread_snapshot` before invoking the model. On subsequent thread page
loads, the UI compares each Q&A's `thread_snapshot` against the
current member set and per-member message counts, surfacing the
diff as the inline staleness tag described in the Thread Page section.

**Persistence point.** The row is INSERTed at question submission time with
`answer = NULL` and `completed_at = NULL`. On stream completion, `answer`
and `completed_at` are populated via UPDATE. If the stream errors, is
cancelled, or the server restarts mid-stream, the row remains with
`answer = NULL` — the user's question text is preserved rather than lost.
On a thread page reload, the UI renders rows with NULL `answer` as
"Did not complete — retry?", offering re-submission against the same
thread context.

**Cascade behavior.** When the thread root conversation is hard-deleted,
its Q&A history is removed via the foreign-key cascade. The history has
no value separated from the source conversations, so this is intentional
rather than a soft-delete hole.

Lookup query: `SELECT … FROM thread_qa WHERE root_conv_id = ? ORDER BY
created_at`.

## Kickstart (REQ-THR-007, REQ-THR-008)

Kickstart is implemented by invoking the seeded-conversations creation flow
(REQ-SEED-001, REQ-SEED-003, REQ-SEED-004) with these inputs derived from
the source Q&A and thread:

| Input | Value |
|---|---|
| `cwd` | The thread root conversation's `cwd` |
| `project_id` | The thread root conversation's `project_id` |
| `conv_mode` | The thread root conversation's `conv_mode` |
| Draft prompt | The Q&A answer text, formatted as a quoted block followed by a separator and a cursor, so the user can extend with their new-direction question before sending |
| `seed_parent_id` | The thread root conv ID |
| `seed_label` | `"Q&A: <question excerpt>"` (truncated to a display-appropriate length) |

The new conversation is created with no `continued_in_conv_id` predecessor
on any thread member, satisfying the chain-vs-offshoot distinction in
REQ-THR-008. **It is an offshoot member of the source thread**: the
membership rule (chain root has `seed_parent_id == thread root`) places
it on an offshoot sub-chain rooted at the new conversation. The source
thread's forward walk picks up the new conversation via the offshoot-root
query (`SELECT id FROM conversations WHERE seed_parent_id = ?`), the
source thread's member list and sidebar block include it, and the Q&A
context bundling pulls in its content alongside the main line.

If the kickstarted conversation is itself later continued via the
existing "continue in new conversation" flow, those continuations extend
the same offshoot sub-chain (visible as further indentation under that
offshoot in the source thread's UI). They remain part of the source
thread, not a new thread.

Lineage display on the kickstarted conversation's own detail page is
provided by REQ-SEED-003's existing `seed_parent_id` breadcrumb; visual
styling distinguishes it from the continuation breadcrumb in the same
view (continuation breadcrumbs are tagged "continued from"; kickstart
breadcrumbs are tagged "kickstarted from thread"). The breadcrumb
complements the source thread's member-list affordance — it lets the
user navigate from a single member detail page back to the thread, while
the source thread's member list lets them see the offshoot in context
alongside its siblings.

## Out-of-Scope Properties

These are properties this design intentionally does not provide, in addition
to the user-visible non-requirements listed in `requirements.md`:

- **No `threads` table.** Derived from the conversation graph. Adding one
  would be necessary only if post-hoc membership editing is introduced.
- **No follow-up Q&A context layering.** REQ-THR-006 prohibits prior Q&A in
  the model's context.
- **No new system-context-attachment mechanism on conversations.**
  Kickstart's draft-prompt mechanism (REQ-SEED-001) carries the seed
  content; it is visible to the user before submission.
- **No cross-thread linking.** No requirement defines it; the kickstart
  breadcrumb is the only lineage link, and it points from kickstarted conv
  back to source thread.

## Cross-Spec References

- `specs/bedrock/` — owns `MessageType::Continuation` and the conversation
  state machine; threads consume continuation summary messages as inputs
- `specs/seeded-conversations/` — owns `seed_parent_id`, `seed_label`, and
  the review-first draft prompt mechanism; kickstart reuses all three
- `specs/projects/` — owns `project_id` scoping; kickstart inherits the
  source thread's project
- `specs/sse_wire/` — owns the SSE streaming infrastructure used for Q&A
  token streaming
- `specs/ui/` — owns the conversation detail page that renders the
  kickstart breadcrumb (REQ-SEED-003-style lineage from kickstarted conv
  back to source thread)
