# Phoenix Chains — Design

## Architecture Overview

Chains are a *derived navigation primitive* over Phoenix's existing
`conversations.continued_in_conv_id` graph. Edges in that graph are
linear handoffs: context continuation, and managed Explore approval into
a fresh Work conversation. The only schema change to
`conversations` is a single nullable `chain_name TEXT` column carrying
the user-set name on chain root conversations. One new persistence
table: `chain_qa` for Q&A history. No `chains` table; membership is
computed by walking the continuation chain.

The Q&A surface is a single per-chain persistent UI history. Each
question is answered by a **read-only agentic loop**: a fresh agent,
given the chain's content through the product-wide retrieval primitive
(`specs/conversation-retrieval/`) as a scope-bound search tool plus a
scope-bound paged read tool, iterates — search, read promising members
in full, search again — until it can answer, then streams the answer.
The agent's scope is bound to the chain (it cannot read outside it) and
it has no state-mutating tools. Each question is a fresh run with no
memory of prior Q&A (REQ-CHN-006). Q&A history persists in `chain_qa`.

The chain page also carries the work-scope dock (`specs/work-scope-ui/`
REQ-WSUI-009), to which REQ-CHN-008 adds the chain's **work identity** —
worktree/branch/task and PR health — alongside the dock's runtime-resource
rows, so the page shows not just which conversations the chain contains
but what unit of work they share and its PR status.

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

**Work identity on the work-scope dock (REQ-CHN-008).** The chain page
already hosts a **work-scope dock** (`specs/work-scope-ui/` REQ-WSUI-009):
`ChainWorkScopeDock` resolves the chain root's `work_scope_key` (one
`get_conversation`) and renders the shared `WorkScopePanel`, showing the
scope's live runtime resources (bash / tmux / browser) from
`WorkScopeInventory`. That dock answers "what is *running* on this work."
REQ-CHN-008 adds the complementary "what *unit of work* is this" facet to
the same surface, addressed by the **same single `work_scope_key`** (a
chain's members share one scope; at most one is non-terminal, `specs/projects/`
REQ-PROJ-025 — so no per-member fan-out):

- **Work identity** — worktree path, branch, base branch, and the task
  (id + title) for Managed work — from the members' `ConvMode` git
  metadata.
- **PR health** — `display_state` (open / draft / merged / closed),
  checks, and feedback-freshness — from the existing **PR-status /
  feedback pipeline** that drives the StateBar (work-lifecycle
  REQ-WL-003, pr-association REQ-PRA-001/REQ-PRA-002). `work_scope_pr_associations` carries PR identity
  and state only; checks come from the live PR-status path (e.g.
  `gh pr checks`) and freshness from `work_scope_pr_feedback_baselines`
  compared against current feedback. Reading the association row alone
  would omit checks and stale the freshness marker.

**Not part of `WorkScopeInventory`.** This facet is deliberately *not*
folded into the inventory snapshot. `WorkScopeInventory` is a
full-snapshot read-projection over the in-memory runtime registries,
re-broadcast on a registry state change (`specs/work-scope-ui/`
REQ-WSUI-001/-007). Externally-polled PR state and durable git metadata
have a different freshness model (a PR transition is not a registry
event), so they ride the PR-status pipeline and `ConvMode`, rendered as
an additional block on the dock rather than as inventory fields.

When the chain has no managed work scope (e.g. a chain of Direct
conversations with no worktree), the identity block says so rather than
rendering empty worktree/branch/PR fields. No new persistence: the facet
reuses existing git metadata and the existing PR-status/feedback pipeline.

**Placement** of the dock on the chain page (REQ-WSUI-009 notes its
standalone right-adjacent placement as provisional pending a layout
reconciliation) is owned by `specs/work-scope-ui/`; REQ-CHN-008 governs
the *content* of the identity facet, not where the dock sits.

**Layout (two-column):**

- **Left:** member conversations rendered as cards in chain order
  (root → latest). Each card shows title, position label
  (root / continuation / latest), date, and message count. The latest
  active member's card is visually emphasized (badge or bold) so the
  user can see at a glance which conversation to click for
  resume-style work. Clicking any member card navigates to that
  conversation's detail page in a state ready for the user to continue
  working (input focused, history loaded).
- **Right:** Q&A panel rendered as a vertical scratchpad of pair
  cards. Each pair card has two labeled rows — `Q:` and `A:`. Index 0
  is always an **active pair card**: `Q:` is an autofocused textarea
  with an Ask button; `A:` is a "waiting for question" placeholder.
  Below the active card, in-flight pairs (this tab's just-submitted
  questions, currently streaming) and persisted pairs render in
  reverse chronological order — the most recent pair sits just below
  the active card; the oldest pair is at the bottom. On submit, the
  just-submitted pair drops in at index 1 (newest in-flight just below
  active), the active textarea is cleared, and focus returns to the
  active textarea so the user can immediately type the next question
  without waiting for the answer. Multiple concurrent in-flight pairs
  are valid; each demuxes its own tokens by `chain_qa_id`. Pair cards
  do not move when their state transitions (streaming → completed /
  failed / abandoned); they stay where they were inserted.

  **Q&A entry independence (REQ-CHN-006).** Pair cards are explicitly
  the visual pattern that satisfies REQ-CHN-006: each pair is a
  self-contained record with explicit Q/A label gutter, clear border,
  and a vertical gap from siblings — no visual ligatures (no
  thread/reply lines, no avatar continuity, no indenting follow-ups).
  The active card has the same shape as past pairs (just unfilled),
  which structurally communicates that the next question creates a
  new pair rather than continuing a thread. The active textarea is
  always empty after submission; it does not preserve drafts and does
  not "thread" into the previous answer.

  **Q&A freshness indicator (REQ-CHN-005).** Each answer is generated
  against the chain's live content at submission time (the agent reads
  the current index and messages — there is no during-answer snapshot to
  be stale against). But a *stored* answer in the history can still
  predate later chain activity, and the user needs to see that at a
  glance. So each Q&A card displays a subtle inline freshness tag when
  the chain has advanced since the answer was produced — e.g., "chain has
  grown since this answer: 3 → 5 conversations" or "27 messages now, 18
  when answered". It is computed from two cheap integers recorded on the
  Q&A row at answer time (`chain_members_at_answer`,
  `chain_messages_at_answer`) compared against current chain state. This
  is an *age-of-answer* signal, not a correctness-snapshot the answer was
  computed against; no JSON parallel representation, no per-member walk on
  render.

## Chain Name Storage (REQ-CHN-007)

A new nullable column on `conversations`:

```sql
ALTER TABLE conversations ADD COLUMN chain_name TEXT;
```

Set only when the conversation is the root of a chain AND the user has
explicitly named it — either by typing a name inline (REQ-CHN-007) or by
invoking the regenerate action (REQ-CHN-010), both of which are explicit
naming acts initiated by the user. NULL means "use the conversation's
title as the displayed chain name." This keeps naming derived-from-title
by default while letting the user override. `chain_name` holds whatever
the user last committed, typed or regenerated; there is no
source-discriminator column distinguishing the two, because nothing reads
the name's provenance — both paths write the same field.

**Why on `conversations` rather than a new `chains` table:** the chain
root conv ID already serves as the chain's identity. Adding a column
to the root is the smallest change that supports REQ-CHN-007. A
separate `chains` table would add a join for every chain-list render
with no offsetting benefit, and would create a denormalized
membership-vs-conversations integrity surface to maintain.

For non-root conversations (continuation members), `chain_name` is
ignored at read time. Setting it on a non-root conversation has no UI
effect; the API enforces `chain_name` writes only on the chain root.

## Chain Name Regeneration (REQ-CHN-010)

**Shared naming mechanism.** Both LLM-driven names in Phoenix come from
`crate::title_generator`: it calls a cheap model under a short timeout
(seconds, not the full request budget) with a stable prompt-cache key, so
repeated calls reuse the cached prompt prefix, and falls back gracefully
when the model errors or times out (returning no name rather than a
fabricated one). The generator has two modes:

- **Create-time conversation slug** — a kebab-case, lowercase slug derived
  from a single conversation's first user message. Conversation
  create-time naming reuses this same generator; that path is not
  otherwise specified under chains, but it is named here so a reader knows
  the mechanism's home.
- **Chain-name regeneration** — a prose display name summarizing the first
  user message of *every* chain member, used as the `chain_name` override.

**Regenerate flow.** When the user invokes regenerate on a chain:

1. Walk the chain's members forward from the root via the existing
   `continued_in_conv_id` recursive CTE (the same walk used for membership
   and freshness).
2. Take each member's first user message, in chain order.
3. Generate a prose display name summarizing those messages via
   `title_generator`.
4. On success, persist the name through the existing chain-name write path
   (the same `conversations.chain_name`-on-root update REQ-CHN-007 uses)
   and return the updated chain view.
5. On generation failure or timeout, the stored name is left untouched —
   no partial or empty name is written — and the caller surfaces that
   regeneration did not succeed.

The action gates on chain membership (members ≥ 2): a single conversation
is not a chain (REQ-CHN-002) and has no chain name to regenerate.

**Format distinction.** The create-time slug and the regenerated chain
name are different fields with different validity rules. The slug is
kebab-case/lowercase because it *is* a slug; the regenerated chain name is
a prose display string subject to the same length cap as a typed chain
name (REQ-CHN-007), not slugified. They share only the generator, not the
output shape.

## Q&A Backend (REQ-CHN-001, REQ-CHN-004, REQ-CHN-006, REQ-CHN-009)

**A read-only agentic loop.** Each question runs a fresh agent whose
toolset is two scope-bound tools from `specs/conversation-retrieval/`:

- `search_conversations { query }` — ranked retrieval scoped to the
  chain (the host binds the chain; the scope resolves the chain's members
  live per call, REQ-RET-008);
- `read_conversation { conversation_id, cursor?, max_bytes? }` —
  byte-budgeted, paged full-content read of a chain member, including
  full tool-result bodies (REQ-RET-008).

The agent is seeded with the question, a short instructional system
prompt (answer from chain content; say so when the content does not
support a confident answer), and a cheap **chain skeleton** (member
titles + trailing continuation summaries) for orientation — *not* the
full bundled transcript. It then iterates: search → page through
promising members in full → search again → answer. A first-pass
retrieval miss is recoverable because the agent can search again or read
more, rather than being capped by a single up-front context guess.

**Read-only and scope-bound by construction.** The agent is given no
state-mutating tool (no bash, patch, or worktree access), so "read-only"
is a property of the toolset, not a runtime flag. Both tools are bound to
the chain by the host; the model supplies only a query (or a
target conversation already in scope) and cannot widen beyond the chain
(REQ-RET-008). This is the same agent that, bound to the `Global` scope,
would serve an application-wide Q&A surface — only the bound scope differs.

**No prior Q&A in context (REQ-CHN-006).** Each question is a fresh agent
run; it sees none of the chain's prior questions or answers. The run may
iterate internally, but it carries no cross-question memory, so the tenth
question is answered against the chain's live content exactly as the
first was — no drift from the model's own earlier answers. Cost and
latency now scale with a question's difficulty (how much the agent must
search/read) rather than with chain size, bounded by a fixed **turn
cap**: the loop performs at most N tool iterations before it must answer
with what it has.

**Implementation: a bespoke bounded loop, not the conversation runtime.**
The loop lives in the Q&A module and calls `LlmService::complete_streaming`
with the two tools; on a tool-use response it executes the (pure
read-only DB) tools, appends results, and calls again; on a final text
answer or the turn cap it finalizes. It does **not** reuse the main
conversation runtime: `chain_qa` already owns its lifecycle, streaming,
and persistence (below); the tools are pure reads needing none of the
runtime's sandbox/worktree/state-machine/sub-agent machinery; and the
Q&A is not a `Conversation` (it spans a chain, has no worktree, persists
to `chain_qa`). See `specs/conversation-retrieval/` design for the full
rationale.

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

**Streaming discipline: only the final answer turn is published.** The
agentic loop's intermediate turns can emit text deltas (the model
narrating "I'll search for…") alongside their tool calls. Those deltas
are consumed by the loop but **not** broadcast — during tool iterations
subscribers see the pre-token indicator, not scratch narration. Only the
final answer turn (a text response with no tool-use) streams to the
broadcaster and is persisted, so from the user's side the visible
behavior is identical to a one-shot answer: "working…", then the answer
streams once the agent commits to it.

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
| `chain_members_at_answer` | INTEGER NOT NULL | Number of chain members when this question was answered (for the age-of-answer freshness tag; see Freshness computation below) |
| `chain_messages_at_answer` | INTEGER NOT NULL | Total message count across all chain members when this question was answered |
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
and the two chain-size freshness integers captured. On stream completion,
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
| `completed` | Full answer, with the freshness tag if the chain has advanced since the answer was produced |
| `failed` | Question + failure indicator + "Re-ask?" affordance; partial answer rendered if `answer` is non-NULL |
| `abandoned` | Question + "Did not complete — re-ask?" affordance |

**Freshness computation.** Per-conversation message count in Phoenix is
**not a stored column** — `Conversation::message_count` is computed at
load time via a correlated subquery (`(SELECT COUNT(*) FROM messages m
WHERE m.conversation_id = c.id)`, see `src/db.rs`). When a question is
answered, the backend (a) walks the chain's members forward via the
recursive CTE on `continued_in_conv_id`, (b) loads each member as a
`Conversation` (which carries its query-time `message_count`), and
(c) records `chain_members_at_answer = chain_members.len()` and
`chain_messages_at_answer = chain_members.iter().map(|c|
c.message_count).sum()` on the row. On chain page load, the UI compares
each Q&A row's two integers against the current chain state (computed the
same way) and, when the chain has grown, surfaces the difference as the
inline freshness tag (REQ-CHN-005). This is an *age-of-answer* signal:
the answer was generated against live content at the time, and these
integers tell the user only whether the chain has moved on since — not
that the answer was computed against a stale snapshot. Two integers
replace what would otherwise be a JSON snapshot — same user-visible
signal, no parallel representation of conversation graph state.

**Lifecycle and cascade behavior.**

- **Hard delete of chain root.** When the chain root is hard-deleted
  (`Database::delete_conversation`), `chain_qa` rows are removed via
  the foreign-key cascade. The history has no value separated from
  the source conversations.
- **Archive of chain root.** Phoenix's user-facing default is *archive*
  (`UPDATE conversations SET archived = 1`), not hard delete. Archived
  chain roots **retain** their `chain_qa` rows; the UI hides the chain
  from sidebar grouping (sidebar already filters `archived = 0`) and
  the chain page route returns 404 for archived roots. Archive is a
  terminal lifecycle transition — there is no unarchive (REQ-API-006) —
  so the retained rows keep the archived chain's Q&A history readable for
  inspection, not resumption.
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
- **No bundled-context or summary cache for Q&A.** The agent reads chain
  content live through the retrieval primitive per question; nothing
  about the chain's content is pre-bundled or cached for Q&A, so there is
  no cache that could go silently stale against in-place message edits.
- **No tree-shaped chain membership.** Chains are linear; kickstart
  and offshoots are deferred (named in `requirements.md` Future
  Direction).
- **No follow-up Q&A context layering.** REQ-CHN-006 prohibits prior
  Q&A in the model's context.
- **No new work-scope persistence or projection (REQ-CHN-008).** The work
  identity facet reads existing `ConvMode` git metadata and the existing
  PR-status/feedback pipeline; it adds no table, no column, and no field
  to `WorkScopeInventory`. It reuses the chain dock and `work_scope_key`
  that `specs/work-scope-ui/` already provides.

## Cross-Spec References

- `specs/conversation-retrieval/` — owns the scope-filtered message
  retrieval primitive and the scope-bound search/read tools that the
  chain Q&A agent drives (REQ-CHN-009)
- `specs/work-scope-ui/` — owns the chain page's work-scope dock
  (`ChainWorkScopeDock` / `WorkScopePanel`, REQ-WSUI-009), the
  `work_scope_key` resolution, and `WorkScopeInventory` (runtime
  resources). REQ-CHN-008 adds the work-identity + PR-health facet to that
  same dock; `specs/wake-contracts/` is the agent-facing twin of the same
  per-scope state
- `specs/bedrock/` — owns `MessageType::Continuation` and the
  conversation state machine; chains consume continuation summary
  messages for the orientation skeleton and as continuation edges
- `specs/projects/` — owns `project_id` scoping, the one-non-terminal-
  conversation-per-scope invariant (REQ-PROJ-025) the single-scope-key
  query relies on, and the PR-status/feedback pipeline
  (work-lifecycle REQ-WL-003, pr-association REQ-PRA-001/REQ-PRA-002) the work-identity facet reads (REQ-CHN-008);
  chain membership extends projects' conversation grouping with
  continuation-aware collapsibility
- `specs/sse_wire/` — owns the SSE streaming infrastructure used for
  Q&A token streaming
