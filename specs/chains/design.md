# Phoenix Chains — Design

## Architecture Overview

Chains are a *derived navigation primitive* over Phoenix's existing
`conversations.continued_in_conv_id` graph. The only schema change to
`conversations` is a single nullable `chain_name TEXT` column carrying
the user-set name on chain root conversations. One new persistence
table: `chain_qa` for Q&A history. No `chains` table; membership is
computed by walking the continuation chain.

The Q&A surface is a single per-chain persistent UI history with
stateless per-question invocations of a mid-tier model. Q&A scope is
all members of the chain (root → leaf). Bundling architecture: each
member contributes one context block (continuation summary for
non-leaf members; transcript or in-process summary for the leaf).

## Chain Identity and Membership (REQ-CHN-002)

A chain is identified by its **root conversation ID**: the oldest
ancestor in the `continued_in_conv_id` chain.

**Single-successor invariant.** Continuation edges form linear chains.
`conversations.continued_in_conv_id` admits at most one successor per
conversation, enforced schema-side by the column being scalar and
application-side by `Database::continue_conversation`'s idempotent
`AlreadyContinued` outcome (`src/db.rs`). Chains are linear; the
design relies on this invariant.

**Forward walk via recursive CTE.** Loading members from root in a
single query:

```sql
WITH RECURSIVE chain(id, next_id, depth) AS (
    SELECT id, continued_in_conv_id, 0
    FROM conversations WHERE id = ?
    UNION ALL
    SELECT c.id, c.continued_in_conv_id, chain.depth + 1
    FROM conversations c JOIN chain ON c.id = chain.next_id
)
SELECT id, depth FROM chain ORDER BY depth;
```

**Backward walk** (any member → root) uses the inverse-edge analog
recursive CTE.

A conversation is a chain member iff the chain length (root through
leaf) is ≥ 2. Single conversations are not chains.

## Sidebar Grouping (REQ-CHN-002)

Conversations in the sidebar are sorted `ORDER BY updated_at DESC`
(`Database::list_conversations`, `src/db.rs`). Members of a long-lived
chain are not consecutive in this sort — unrelated conversations from
in-between sit between them. Sidebar grouping performs **chain-block
extraction**:

1. The sidebar query annotates each conversation with its chain root
   conv ID (or `null` if standalone).
2. Conversations belonging to the same chain are extracted from the
   flat recency list into a single collapsible block.
3. Each chain block is positioned at the recency rank of its
   most-recent member, so a chain with recent activity rides at the
   top.
4. Within the block, members are listed in chain order (root → latest)
   independent of their individual `updated_at` values.
5. Standalone conversations remain interleaved by recency between
   chain blocks.

The block header shows the chain's name (REQ-CHN-007). Each chain
block defaults to expanded; expand/collapse state is not persisted
across navigations.

## Chain Page (REQ-CHN-003, REQ-CHN-005, REQ-CHN-007)

Route: `/chains/:rootConvId`. The route is deep-linkable, supports
browser back/forward, and survives refresh.

**Page header.** Displays the chain name as an inline-editable element
(REQ-CHN-007). Click to enter edit mode (text input pre-populated with
the current name); Enter or blur commits via an API call that updates
`conversations.chain_name` on the root conversation; Esc cancels.

**Layout (two-column):**

- **Left:** member conversations rendered as cards in chain order
  (root → latest). Each card shows title, position label
  (root / continuation / latest), date, and message count. The latest
  active member's card is visually emphasized (badge or bold) so the
  user can see at a glance which conversation to click for
  resume-style work. Clicking any member card navigates to that
  conversation's detail page in a state ready for the user to continue
  working (input focused, history loaded).
- **Right:** Q&A panel. Input box anchored at the bottom of the panel.
  Q&A history fills the area above the input, in chronological order
  (oldest at the top of the scroll region, most recent immediately
  above the input). Streaming answers render in place into the slot
  just above the input, flowing downward toward the input as tokens
  arrive — older Q&A above is not displaced.

  **Q&A entry independence (REQ-CHN-006).** Each Q&A entry renders as
  a self-contained card — clear border, vertical gap from siblings,
  no visual ligatures (no thread/reply lines, no avatar continuity,
  no indenting follow-ups). This is a deliberate visual signal that
  questions are answered independently against the chain's content
  rather than as a continuous conversation. The input box is always
  empty after submission; it does not preserve drafts and does not
  "thread" into the previous answer.

  **Q&A snapshot indicator (REQ-CHN-005).** Each Q&A card displays a
  subtle inline tag indicating chain state at answer time when it
  differs from current state — e.g., "answered when chain had 3
  conversations (now 5)" or "answered when 18 messages had been
  written (now 27)". Computed from two integer columns on the Q&A
  row (`snapshot_member_count`, `snapshot_total_messages`) compared
  against current chain state. No JSON parallel representation, no
  per-member walk on render.

## Chain Name Storage (REQ-CHN-007)

A new nullable column on `conversations`:

```sql
ALTER TABLE conversations ADD COLUMN chain_name TEXT;
```

Set only when the conversation is the root of a chain AND the user has
explicitly named it. NULL means "use the conversation's title as the
displayed chain name." This keeps naming derived-from-title by default
while letting the user override.

**Why on `conversations` rather than a new `chains` table:** the chain
root conv ID already serves as the chain's identity. Adding a column
to the root is the smallest change that supports REQ-CHN-007. A
separate `chains` table would add a join for every chain-list render
with no offsetting benefit, and would create a denormalized
membership-vs-conversations integrity surface to maintain.

For non-root conversations (continuation members), `chain_name` is
ignored at read time. Setting it on a non-root conversation has no UI
effect; the API enforces `chain_name` writes only on the chain root.

## Q&A Backend (REQ-CHN-001, REQ-CHN-004, REQ-CHN-006)

**Per-question model invocation receives:**

1. The chain's bundled context (defined below)
2. The user's current question
3. An instructional system prompt directing the model to answer from
   the provided context only and to indicate uncertainty when the
   context does not support a confident answer

The invocation does **not** receive prior Q&A history from the same
chain. This is what makes REQ-CHN-006's user-visible property hold:
each question is answered against the canonical chain content, not
against the model's own prior answers.

**Context bundling.** For a chain of members `M₁ → M₂ → … → Mₙ`:

| Member kind | Context source |
|---|---|
| **Non-leaf** (`Mᵢ` where `i < n`, continued into a successor) | The trailing `MessageType::Continuation` message at the end of `Mᵢ` itself, persisted by `Effect::persist_continuation_message` during the `AwaitingContinuation → ContextExhausted` transition (`src/state_machine/transition.rs`). Its payload describes the work done in `Mᵢ`. |
| **Leaf** (`Mₙ`, never continued) | Transcript sent directly when the leaf has ≤ 20 messages and approximate token count ≤ 4000; otherwise an on-demand summary generated **in-process** by the same mid-tier model in a pre-step before the main answer call. |

Every non-leaf member carries its own summary on its own tail because
each was continued, and that act generated the summary. The leaf has
no such summary because it was never continued from. The leaf-summary
thresholds are pinned to single shared values across all Q&A
invocations to avoid behavior drift between identical questions.

**Leaf summarization is in-process, not persisted.** When the leaf
requires summarization, the result is held in process memory for the
duration of the Q&A request and discarded after. Subsequent questions
re-summarize. An earlier draft of this design persisted leaf summaries
to a `chain_leaf_summary` table keyed by `(leaf_conv_id,
message_count)`; the persistence was rejected because (a) the
`message_count` cache key cannot detect in-place message mutations
(rewrites, tool-result patches, rolled-back turns), risking
silently-stale cache hits that produce confidently-wrong answers, and
(b) the cross-restart durability benefit is small in a single-user
single-server Phoenix. Re-summarizing per-request is operationally
cheap and never diverges from source-of-truth.

**Model.** A mid-tier model balanced for cost and accuracy (Claude
Sonnet-class as of this writing). The model identifier is set at the
Q&A call site; there is no per-chain or per-user override.

**Streaming.** The Q&A response stream uses Phoenix's existing SSE
token-streaming infrastructure (`specs/sse_wire/`). Phoenix's existing
broadcasters are conversation-scoped (one per `Conversation` runtime,
see `src/runtime.rs`); chain Q&A introduces a new chain-scoped
broadcaster keyed by the chain's `root_conv_id`. Each token event
carries the per-question `chain_qa.id` as a request discriminator so
multiple subscribers (e.g., the same chain page open in two tabs) can
demultiplex concurrent Q&As — a subscriber that submitted question A
does not render tokens from a sibling-tab's question B.

**Chain broadcaster lifecycle.** The chain broadcaster is owned by a
chain-runtime registry (analogous to the existing conversation-runtime
registry) keyed by `root_conv_id`. It is created lazily on the first
Q&A submission for a chain and torn down when (a) the last subscriber
disconnects and there is no in-flight stream, or (b) the chain root is
hard-deleted. Tab disconnects decrement the subscriber count; when it
reaches zero with no in-flight stream the broadcaster is dropped.
In-flight streams keep the broadcaster alive past zero subscribers
until the stream reaches a terminal status (`completed` / `failed`),
so a tab close mid-stream does not orphan the model invocation —
subsequent reads pick up the persisted answer from `chain_qa`.

**Loading UX (REQ-CHN-004).** Two visual states:

- **Pre-token** (request in flight, no tokens yet): a skeleton
  placeholder in the answer slot indicating the model is preparing
- **Streaming** (tokens arriving): incremental render token-by-token

## Q&A Persistence (REQ-CHN-005)

New table `chain_qa`:

| Column | Type | Notes |
|---|---|---|
| `id` | TEXT PRIMARY KEY | UUID; doubles as the per-question request id for streaming demux |
| `root_conv_id` | TEXT NOT NULL | Chain identity; `REFERENCES conversations(id) ON DELETE CASCADE` |
| `question` | TEXT NOT NULL | User's submitted question |
| `answer` | TEXT NULL | Final assembled answer once the stream completes; may contain a partial string for `failed` rows; NULL for `in_flight` and `abandoned` |
| `model` | TEXT NOT NULL | Model identifier used for the answer |
| `status` | TEXT NOT NULL | One of `in_flight`, `completed`, `failed`, `abandoned` |
| `snapshot_member_count` | INTEGER NOT NULL | Number of chain members at question submission time |
| `snapshot_total_messages` | INTEGER NOT NULL | Total message count across all chain members at question submission time (computed at submit; see Snapshot computation below) |
| `created_at` | DATETIME NOT NULL | UTC; set when the question was submitted |
| `completed_at` | DATETIME NULL | UTC; set when status transitions to `completed` |

Index: `CREATE INDEX idx_chain_qa_root ON chain_qa(root_conv_id, created_at)`
so the per-chain lookup query is index-served.

**Status lifecycle.** Each Q&A row passes through these statuses:

- `in_flight`: row inserted at question submission; stream is being
  generated by a live process.
- `completed`: stream finished cleanly; `answer` and `completed_at`
  populated.
- `failed`: stream ended in error before producing a full answer
  (model error, parse failure, network drop). `answer` may contain a
  partial string; `completed_at` remains NULL.
- `abandoned`: stream did not complete and is no longer in flight
  (server restarted, SSE channel closed before completion). Distinct
  from `failed` in that there's no active failure cause — the stream
  was simply orphaned and cannot resume.

**Persistence point.** The row is INSERTed at question submission time
with `status = 'in_flight'`, `answer = NULL`, `completed_at = NULL`,
and the snapshot counters captured. On stream completion,
`answer`, `completed_at`, and `status = 'completed'` are populated
via UPDATE. On stream error, `status = 'failed'` is set. The user's
question text is preserved across failure modes rather than lost.

**Startup sweep.** On server startup, any `chain_qa` row with
`status = 'in_flight'` is transitioned to `abandoned` (no live
process is generating it). This prevents indefinite "Did not complete"
UI states for rows that are dead.

**UI rendering by status.**

| Status | UI rendering |
|---|---|
| `in_flight` | Streaming render (live) for the originating subscriber; "still working…" placeholder for other subscribers tailing the same chain |
| `completed` | Full answer, with snapshot tag if state has advanced since `created_at` |
| `failed` | Question + failure indicator + "Re-ask?" affordance; partial answer rendered if `answer` is non-NULL |
| `abandoned` | Question + "Did not complete — re-ask?" affordance |

**Snapshot computation.** Per-conversation message count in Phoenix is
**not a stored column** — `Conversation::message_count` is computed at
load time via a correlated subquery (`(SELECT COUNT(*) FROM messages m
WHERE m.conversation_id = c.id)`, see `src/db.rs`). When a question is
submitted, the backend (a) walks the chain's members forward via the
recursive CTE on `continued_in_conv_id`, (b) loads each member as a
`Conversation` (which carries its query-time `message_count`), and
(c) sums the counts in application code: `member_count =
chain_members.len()`, `total_messages = chain_members.iter().map(|c|
c.message_count).sum()`. Both integers are written into the row
before invoking the model. On chain page load, the UI compares each
Q&A row's snapshot integers against the current chain state (computed
the same way) and surfaces the difference as the inline staleness tag
(REQ-CHN-005). Two integers replace what would otherwise be a JSON
snapshot — same user-visible signal, no parallel representation of
conversation graph state.

**Lifecycle and cascade behavior.**

- **Hard delete of chain root.** When the chain root is hard-deleted
  (`Database::delete_conversation`), `chain_qa` rows are removed via
  the foreign-key cascade. The history has no value separated from
  the source conversations.
- **Archive of chain root.** Phoenix's user-facing default is *archive*
  (`UPDATE conversations SET archived = 1`), not hard delete. Archived
  chain roots **retain** their `chain_qa` rows; the UI hides the chain
  from sidebar grouping (sidebar already filters `archived = 0`) and
  the chain page route returns 404 for archived roots, but the rows
  remain so unarchive restores Q&A history along with the chain.
- **Mid-chain hard deletion.** Phoenix's existing schema places no
  `ON DELETE` clause on `conversations.continued_in_conv_id`
  (`src/db/migrations.rs`), so the FK defaults to `NO ACTION` —
  hard-deleting any non-leaf member fails because its predecessor's
  pointer still references it. This is a pre-existing Phoenix
  invariant, not a chains-spec concern; chain Q&A history is
  unaffected because nothing it relies on is broken.

## Out-of-Scope Properties

These are properties this design intentionally does not provide, in
addition to the user-visible non-requirements listed in
`requirements.md`:

- **No `chains` table.** Membership is derived from the conversation
  graph. Adding one would only be necessary if post-hoc manual
  membership editing entered scope.
- **No persisted leaf-summary cache.** Replaced with in-process
  summarization per Q&A request (see Q&A Backend section above for
  the rationale).
- **No tree-shaped chain membership.** Chains are linear; kickstart
  and offshoots are deferred (named in `requirements.md` Future
  Direction).
- **No follow-up Q&A context layering.** REQ-CHN-006 prohibits prior
  Q&A in the model's context.
- **No retrieval-backed Q&A.** Tracked as a future-direction note in
  `requirements.md`.

## Cross-Spec References

- `specs/bedrock/` — owns `MessageType::Continuation` and the
  conversation state machine; chains consume continuation summary
  messages as inputs to the Q&A bundle
- `specs/projects/` — owns `project_id` scoping; chain membership
  extends projects' conversation grouping with continuation-aware
  collapsibility
- `specs/sse_wire/` — owns the SSE streaming infrastructure used for
  Q&A token streaming
