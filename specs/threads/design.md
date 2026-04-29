# Phoenix Threads — Design

## Architecture Overview

Threads are a *derived navigation primitive* over the existing
`conversations.continued_in_conv_id` graph. No new fields are added to the
`conversations` table; no `threads` table is introduced. Persistence is
limited to a new `thread_qa` table for Q&A history.

The Q&A surface is a single per-thread persistent UI history, with stateless
per-question invocations of a mid-tier model. Kickstart reuses the
seeded-conversations mechanism (`specs/seeded-conversations/`) — kickstart is
a special case of seeding where the draft prompt source is a Q&A answer.

## Thread Identity and Membership (REQ-THR-002)

A thread is identified by its **root conversation ID**: the oldest ancestor
in the `continued_in_conv_id` chain.

**Single-successor invariant.** Threads are linear (no branching) because
`conversations.continued_in_conv_id` admits at most one successor per
conversation. This is enforced at the schema level by the column being
scalar, and at the application level by `Database::continue_conversation`'s
idempotent `AlreadyContinued` outcome (`src/db.rs`). The thread design
relies on this invariant; introducing multi-successor branching would
require redefining thread membership semantics.

**Walks via recursive CTE.** Both forward and backward chain walks use a
single recursive CTE rather than N round trips. The forward walk
(root → leaf) is the hot path used to load thread members for the thread
page:

```sql
WITH RECURSIVE chain(id, next_id, depth) AS (
    SELECT id, continued_in_conv_id, 0
    FROM conversations WHERE id = ?
    UNION ALL
    SELECT c.id, c.continued_in_conv_id, chain.depth + 1
    FROM conversations c JOIN chain ON c.id = chain.next_id
)
SELECT id FROM chain ORDER BY depth;
```

The backward walk (any member → root) uses an analogous recursive CTE that
follows the inverse edge `p.continued_in_conv_id = current.id` until a
conversation with no predecessor is reached.

A conversation is a thread member iff the chain length (root through leaf)
is ≥ 2. Single conversations are not threads (REQ-THR-002).

## Sidebar Grouping (REQ-THR-002)

Conversations in the sidebar are sorted `ORDER BY updated_at DESC`
(`Database::list_conversations`, `src/db.rs`). Members of a long-lived
thread are not consecutive in this sort — unrelated conversations from
in-between sit between them. Sidebar grouping therefore performs
**thread-block extraction**:

1. The sidebar query annotates each conversation with its thread root conv
   ID (or `null` if standalone).
2. Conversations belonging to the same thread are extracted from the flat
   recency list into a single collapsible block.
3. Each thread block is positioned at the recency rank of its
   most-recent member (so a thread with recent activity rides at the top).
4. Within the thread block, members are listed in chain order (root → latest)
   independent of their individual `updated_at` values.
5. Standalone conversations remain interleaved by recency between thread
   blocks.

Thread display name is the root conversation's title. Each thread block
defaults to expanded; expand/collapse state is not persisted across
navigations.

## Thread Page (REQ-THR-003, REQ-THR-005)

Route: `/threads/:rootConvId`. The route is deep-linkable, supports browser
back/forward, and survives refresh.

**Layout (two-column):**

- **Left:** member conversations rendered as cards in chain order. Each card
  shows title, continuation index (root / continuation / latest), date, and
  message count. Clicking a card navigates to the conversation detail page.
- **Right:** Q&A panel. Input box anchored at the bottom of the panel.
  Q&A history fills the area above the input, in chronological order
  (oldest at the top of the scroll region, most recent immediately above
  the input). Streaming answers render in place into the slot just above
  the input, flowing downward toward the input as tokens arrive — older
  Q&A above is not displaced. Each Q&A entry shows the question, the
  answer, and a Kickstart button.

## Q&A Backend (REQ-THR-001, REQ-THR-004, REQ-THR-006)

**Per-question model invocation receives:**

1. The thread's bundled context (defined below)
2. The user's current question
3. An instructional system prompt directing the model to answer from the
   provided context only and to indicate uncertainty when the context does
   not support a confident answer

The invocation does **not** receive prior Q&A history from the same thread.
This is what makes REQ-THR-006's user-visible property hold: each question is
answered against the canonical thread content, not against the model's own
prior answers.

**Context bundling.** For a thread of members `M₁ → M₂ → … → Mₙ`:

| Member | Context source |
|---|---|
| `Mᵢ`, `i < n` (non-leaf) | The trailing `MessageType::Continuation` message at the end of `Mᵢ` itself, persisted by `Effect::persist_continuation_message` during the `AwaitingContinuation → ContextExhausted` transition (`src/state_machine/transition.rs`). Its payload describes the work done in `Mᵢ`. |
| `Mₙ` (leaf) | Transcript sent directly when the leaf has ≤ 20 messages and approximate token count ≤ 4000; otherwise an on-demand summary generated by the same mid-tier model in a pre-step before the main answer call. |

Every non-leaf member carries its own summary on its own tail because each
was continued, and that act generated the summary. The leaf has no such
summary because it was never continued from. The leaf-summary thresholds
are pinned to single shared values across all Q&A invocations to avoid
behavior drift between identical questions.

**Leaf-summary cache.** When the leaf requires summarization, the result
is cached in-memory keyed by `(leaf_conv_id, leaf.message_count)`.
Subsequent Q&A invocations on the same thread reuse the cached summary
unless the leaf has received new messages (message count increment
invalidates the entry). The cache is process-local; on server restart it
is empty and the first Q&A regenerates it.

**Model.** Claude Sonnet 4.6. The model identifier is set at the Q&A call
site; there is no per-thread or per-user override.

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

Index: `CREATE INDEX idx_thread_qa_root ON thread_qa(root_conv_id, created_at)`
so the per-thread lookup query is index-served.

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

The new conversation is created with no `continued_in_conv_id` linking back
to it from any thread member, satisfying REQ-THR-008. **It is not a member
of the source thread**: the forward CTE walk from the source thread's root
will not include the kickstarted conversation, the source thread's member
list will not include it, and the source thread's sidebar block will not
list it. The kickstarted conversation is structurally a standalone
conversation; if it is later continued, those continuations form a new
thread rooted at the kickstarted conversation, independent of the source
thread.

Lineage display is provided by REQ-SEED-003's existing `seed_parent_id`
breadcrumb mechanism; visual styling distinguishes it from the in-thread
continuation breadcrumb in the conversation detail view (continuation
breadcrumbs are tagged "continued from"; kickstart breadcrumbs are tagged
"kickstarted from thread").

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
