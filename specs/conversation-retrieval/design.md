# Conversation Retrieval — Design

## Overview

One index over all conversation messages, queried through one
scope-filtered primitive. Chain Q&A and a future application-wide Q&A
are the same call with a different scope. The ranking backend sits
behind a trait so a semantic backend can replace the lexical MVP
without touching callers.

```
                         ┌────────────────────────────┐
 chain Q&A ──────────┐   │  MessageRetriever (trait)  │
 (scope = chain ids) │   │  retrieve(query, scope, k) │
                     ├──▶│                            │──▶ Vec<RetrievedChunk>
 app-wide Q&A ───────┘   │  impl: Fts5Retriever (MVP) │
 (scope = Global)        │  impl: VectorRetriever …   │
                         └─────────────┬──────────────┘
                                       │ reads
                                ┌──────▼───────┐   derived from   ┌──────────┐
                                │  message_fts │ ◀────────────────│ messages │
                                │  (FTS5)      │  index on insert │ (truth)  │
                                └──────────────┘  + startup sweep └──────────┘
```

## The primitive

```rust
pub enum RetrievalScope {
    Conversations(Vec<String>), // chain members
    Global,                     // every conversation
    // future: Project(String), Since(DateTime<Utc>)
}

pub struct RetrievedChunk {
    pub conversation_id: String,
    pub message_id: String,
    pub chunk: ChunkRef,        // identity *within* the message (REQ-RET-006)
    pub message_type: MessageType,
    pub created_at: DateTime<Utc>,
    pub snippet: String,
    pub score: f64,
}

/// Locates a chunk within its message. The MVP indexes one chunk per
/// message, so it is always `ordinal 0` over the whole message; a
/// chunking vector/hybrid backend assigns a distinct ordinal (and
/// optional char range) per chunk. The field is present from the MVP so
/// substituting a chunking backend does not change `RetrievedChunk`.
pub struct ChunkRef {
    pub ordinal: u32,                 // 0 for whole-message (MVP)
    pub char_range: Option<(usize, usize)>, // Some(..) once messages are split
}

#[async_trait]
pub trait MessageRetriever: Send + Sync {
    async fn retrieve(
        &self,
        query: &str,
        scope: RetrievalScope,
        top_k: usize,
    ) -> Result<Vec<RetrievedChunk>, RetrievalError>;
}
```

`RetrievalScope` is the entire difference between chain recall and
application-wide recall (REQ-RET-001). It is an enum, not a stringly
flag, so an unhandled scope is a compile error rather than a silent
fall-through to "global".

## Index shape (REQ-RET-002, REQ-RET-003)

A standalone FTS5 virtual table — *not* an external-content table over
`messages`, because the indexed text is extracted prose, not the raw
`content` JSON column:

```sql
CREATE VIRTUAL TABLE message_fts USING fts5(
    text,                        -- extracted, searchable
    message_id      UNINDEXED,
    chunk_ordinal   UNINDEXED,   -- 0 for whole-message (MVP); per-chunk later
    conversation_id UNINDEXED,
    message_type    UNINDEXED,
    created_at      UNINDEXED,
    content_hash    UNINDEXED     -- fingerprint of source content, for change detection
);
```

`UNINDEXED` columns are stored but not tokenized: they are available to
the `WHERE`/projection without participating in the match, which is
exactly what scope filtering (REQ-RET-007), provenance/chunk identity
(REQ-RET-006), and changed-row detection (below) need. Carrying them on
the FTS row removes any join back to `messages` at query time.
`content_hash` is a fingerprint of the message's stored content captured
at index time; comparing it to the current message's fingerprint is how
reconciliation detects a row whose content changed in place.

**Migration creates the table; Rust performs the backfill (REQ-RET-003).**
The static-SQL migration framework (`Migration { sql: &'static str }`)
creates the empty `message_fts` structure only. It cannot populate the
extracted text, because that text is a Rust-side extraction of typed
`MessageContent` (REQ-RET-004) and the migration would otherwise have to
re-implement that extraction in SQL over the raw JSON `content` column.
The **typed backfill** of existing rows is therefore done by the Rust
startup reconciliation (below), not by the migration. A full rebuild is
`DELETE FROM message_fts` followed by the same Rust re-extraction from
`messages`.

### Keeping the index current

- **On persist.** The message write path extracts text and inserts the
  FTS row(s) in the same logical operation that writes the message, so a
  freshly persisted message is immediately retrievable
  (REQ-RET-003). Insertion is guarded so a re-run for the same
  `(message_id, chunk_ordinal)` does not duplicate a row
  (delete-then-insert on `message_id`, or an existence check).
- **On content mutation.** Message content is not strictly append-only —
  e.g. `Database::update_tool_message_content` rewrites a tool message's
  content after the tool completes. The same write path re-extracts and
  re-indexes (delete-then-insert on `message_id`) whenever stored
  content is updated, so the index never keeps the stale extraction of a
  mutated row.
- **On startup.** A reconciliation sweep (a) indexes any `message_id`
  present in `messages` but absent from `message_fts`, and (b)
  re-indexes any message whose current content fingerprint differs from
  the `content_hash` stored on its FTS row — catching rows that were
  mutated while a prior index shape was in effect, or before the
  on-mutation hook existed. It records a freshness watermark (e.g. the
  max `messages.rowid`/sequence reconciled) so the system can report
  whether the index has caught up (REQ-RET-003, Transparency Contract
  item 3). It is idempotent and safe to run every boot.

Triggers on `messages` are deliberately *not* used to maintain the FTS
table, because the indexed text is a Rust-side extraction of typed
`MessageContent` (REQ-RET-004) that SQL triggers cannot reproduce from
the JSON column. The Rust write path owns extraction (on insert and on
mutation); the startup sweep owns reconciliation (absent ids and changed
fingerprints).

## Text extraction (REQ-RET-004)

Indexing reuses the message-text extraction already used to render
transcripts for model context (the logic behind
`chain_qa::render_leaf_transcript`). Extracting it to a shared function
that both the indexer and the transcript renderer call satisfies
REQ-RET-004's "cannot diverge" clause and removes a parallel
representation. Per `MessageContent` variant:

| Variant | Indexed text |
|---|---|
| `User` | message text |
| `Agent` | concatenated `Text` blocks (tool-use blocks omitted) |
| `System` | message text |
| `Error` | error message |
| `Continuation` | continuation summary |
| `Skill` | `/{name} {trigger}` |
| `Tool` | a **bounded searchable excerpt** of the result body (head-truncated to a fixed cap, e.g. the first ~2 KB), prefixed with a compact size marker (`(tool result: N chars)`) |

Tool results are **content-bearing and must be searchable**: the user
story explicitly asks to find actual conversation content, and the
answer to a query is frequently a term that appears only in a build log,
grep output, sub-agent output, or a file path — all of which live in
tool results. Indexing only a `(tool result: N chars)` marker would make
those terms unretrievable, so the index stores a bounded excerpt (a
head-truncated slice capped at a fixed size) rather than a bare marker.
The cap keeps a single huge result from dominating the FTS table while
still exposing its leading domain terms to BM25. (A future chunking
backend can index the whole result as multiple chunks; the MVP caps to
one bounded excerpt per tool message.)

This table is the **index/orientation** extraction — optimized to make
content *findable* within a size budget. The `read_conversation` tool
returns the **full** result body (paged), because once the agent has
found a promising conversation it must be able to read the part beyond
the indexed excerpt (REQ-RET-004; see Tool exposure). The
shared-extraction "cannot diverge" guarantee is between the index and the
orientation skeleton, not between the index and the read path.

## The MVP backend: `Fts5Retriever` (REQ-RET-005)

```sql
-- scope = Conversations([a, b, c])
SELECT text, message_id, chunk_ordinal, conversation_id, message_type, created_at,
       bm25(message_fts) AS score
FROM message_fts
WHERE message_fts MATCH :query
  AND conversation_id IN (:a, :b, :c)
ORDER BY score          -- bm25() is ascending = most relevant first
LIMIT :top_k;

-- scope = Global: drop the conversation_id predicate
```

The scope predicate is part of the ranked query (REQ-RET-007), so
`LIMIT :top_k` yields the top *in-scope* results, never a global top_k
thinned by a post-filter. The projection includes `chunk_ordinal` so
every row carries its full provenance and chunk identity (REQ-RET-006):
the backend maps it into `RetrievedChunk.chunk` (`ChunkRef { ordinal,
char_range }` — ordinal 0 / `None` range for the one-chunk-per-message
MVP), so callers can cite and de-duplicate without the result shape
changing when a chunking backend lands. `snippet(message_fts, …)`
provides the display snippet; `bm25()` provides the score.

FTS5 is available because `crates/phoenix-db` builds SQLite via
`libsqlite3-sys` with the `bundled` feature, which compiles in FTS5. No
new dependency, no embedding provider, no network on the query path
(REQ-RET-005 rationale).

Query strings are passed through FTS5's query syntax defensively: user
text is quoted/escaped so punctuation in a question is treated as terms,
not as FTS5 operators.

## The future backend: vector / hybrid

A `VectorRetriever` (or `HybridRetriever`) implements the same trait.
It would:

- split long messages into chunks (the MVP's one-chunk-per-message is a
  special case),
- embed chunks via an embedding provider (a capability the Anthropic
  provider lacks, so this forces a provider decision — deferred with
  the backend),
- store vectors either via the `sqlite-vec` extension or as BLOBs scored
  by brute-force cosine in Rust (trivial at the scale of one user's
  message history),
- optionally rank-fuse with the BM25 results (reciprocal-rank fusion).

Because callers only ever call `retrieve(query, scope, top_k)`, this is
a substitution, not a refactor (REQ-RET-005).

## Tool exposure (REQ-RET-008)

The first consumer is not a one-shot bundler but a read-only **agentic**
Q&A loop (`specs/chains/` REQ-CHN-009). It drives two scope-bound tools:

```rust
// Constructed by the host with the scope already baked in.
// The model supplies only the highlighted argument(s).

search_conversations { query }
  -> retriever.retrieve(query, bound_scope(), top_k)   // ranked chunks

read_conversation { conversation_id, cursor?, max_bytes? } // must be in bound scope
  -> { content, next_cursor: Option }                      // one size-bounded page
```

**Scope is bound by the host, resolved live (REQ-RET-008 + liveness).**
The host fixes *what* the agent may reach, not a frozen list. For chain
Q&A it binds the **chain root**; `bound_scope()` resolves
`Conversations(chain_members_forward(root))` at each tool call, so a
member added mid-run (a rare continuation while the agent is searching)
becomes visible rather than being excluded by a start-of-run snapshot —
consistent with REQ-CHN-009's "reads current state, no snapshot" claim.
The model still never sees a scope argument and cannot widen beyond the
root's chain. For a future application-wide Q&A the bound scope is
`Global`. `read_conversation` validates its `conversation_id` against the
live bound scope and refuses anything outside it (for `Global`, any
conversation is in scope).

**`read_conversation` returns full content, bounded by a byte budget
(REQ-RET-004, REQ-RET-008).** Unlike the index/orientation text — which
keeps only a capped excerpt of tool results for clean ranking —
`read_conversation` returns the **full** content, including complete
tool-result bodies, build logs, and sub-agent output, because that is
often exactly where the answer lives.

The page is bounded by **total size, not message count.** Each call
returns at most `max_bytes` of content (a host default if the model
omits it) and a `next_cursor` when more remains; the agent pages forward
until `next_cursor` is `None`. The cursor is `(sequence_id,
char_offset)`, so the bound holds **even within a single oversized
message**: a 5 MB build-log tool result is returned as successive
`max_bytes`-sized slices across calls (cursor advances the char offset
within that `sequence_id`), then moves to the next message. A
message-count-only cursor would not satisfy REQ-RET-008 — one giant
message could still blow the next model call's context — so the budget
is in bytes and continuation is intra-message. This is the deliberate
full-content read path that REQ-RET-004 distinguishes from the capped
index excerpt: different purposes (read ground truth vs. rank), so
different (separately-typed) renderings, not a parallel representation of
the same thing.

## Consumer: chain Q&A loop

`specs/chains/` REQ-CHN-009 specifies the chain side. The Q&A run:

1. Construct `search_conversations` + `read_conversation` bound to the
   **chain root**; the bound scope resolves
   `Conversations(chain_members_forward(root))` live at each tool call
   (so a mid-run continuation is visible, not snapshotted).
2. Run a read-only agent (no bash, no patch, no worktree) seeded with
   the question and a cheap **chain skeleton** (member titles + trailing
   continuation summaries) for orientation. The agent iterates:
   search → page through promising members' full content → search
   again → answer.
3. Stream **only the final answer turn** over the existing chain-scoped
   SSE broadcaster; intermediate tool-use turns are not streamed to the
   user (see streaming discipline below).

This replaces `bundle_chain_context`'s summaries-only, single-shot
assembly (trailing continuation summary per non-leaf member; leaf
transcript or in-process leaf summary). Consequences:

- The leaf-direct-vs-summary asymmetry disappears — the agent can read
  any member in full, on demand.
- A first-pass retrieval miss is recoverable — the agent searches again
  or reads more, rather than being capped by one up-front guess.
- Snapshot staleness disappears structurally: every run reads the
  current index and current messages, so there is no "answered at an
  earlier snapshot" state. REQ-CHN-009 supersedes the snapshot-staleness
  machinery of REQ-CHN-005.

### Design decision: a bespoke bounded loop, not the conversation runtime

The agentic Q&A is a **small bounded search/read/answer loop in the Q&A
module**, not a reuse of Phoenix's main conversation runtime.

The loop: call `LlmService::complete_streaming` with the two scope-bound
tools offered; if the model returns tool-use blocks, execute them (pure
read-only DB lookups), append the results, and call again; when the
model returns a final text answer — or a fixed turn cap is reached —
stream it and finalize. The final answer streams over the existing
chain-scoped broadcaster exactly as the one-shot answer does today.

Why bespoke rather than the conversation runtime:

- **Q&A already is its own mini-runtime.** `chain_qa` already owns a
  detached spawned task, an `in_flight → completed | failed` lifecycle,
  a chain-scoped SSE broadcaster with a `chain_qa_id` demux key, and a
  DB finalize step. Adding a tool-call loop around its existing
  streaming call is an incremental extension of code that already runs,
  not new infrastructure.
- **The tools are pure reads.** `search_conversations` and
  `read_conversation` are read-only DB queries with no side effects, so
  none of the conversation runtime's machinery — sandbox, worktree,
  permission gating, the 18-state lifecycle, sub-agent
  `AwaitingSubAgentResult` plumbing, message persistence to
  `conversations`/`messages` — is needed. Reusing the runtime would mean
  standing up all of it only to disable most of it.
- **Q&A is not a conversation.** It has no single parent conversation
  (it spans a chain), no worktree, and persists to `chain_qa`, not to
  `conversations`. Forcing it into the `Conversation` + worktree +
  state-machine shape is an impedance mismatch with nothing gained.
- **Read-only by construction.** A loop whose only tools are two DB
  reads structurally cannot mutate state — the read-only guarantee of
  REQ-CHN-009 is a property of the toolset, not a runtime configuration
  that could be misconfigured.

The cost — reimplementing a thin slice of the tool-execution loop
(parse tool-use, dispatch, append tool-result, re-call) — is small and
contained because the tool set is two trivial reads and the turn cap
bounds it. The retrieval primitive and the two tools are identical to
what a runtime-reuse approach would need; only the host loop differs.

A fixed **turn cap** bounds cost and latency (supporting REQ-CHN-006's
stability-as-history-grows property): the loop runs at most N
search/read iterations before it must answer with what it has.

### Streaming discipline: only the final turn reaches the user

`complete_streaming` emits text deltas for *every* turn, including the
intermediate tool-use turns where the model may narrate its plan
("I'll search for the migration discussion…") alongside its tool calls.
Forwarding those deltas onto the chain Q&A broadcaster would interleave
planning chatter with the answer. The loop therefore distinguishes
**intermediate turns** from the **final answer turn**:

- A turn that ends in tool-use is intermediate. Its text deltas are
  consumed by the loop (and may be logged) but are **not** published to
  the chain-scoped broadcaster — the user sees an in-flight indicator,
  not the model's scratch narration.
- The first turn that returns a final text answer with no tool-use is
  the answer turn. *Its* deltas stream to the broadcaster, exactly as
  the one-shot answer streams today, and are what `finalize` persists.

So mid-loop the user sees "working…", then the answer streams once the
agent commits to it. This keeps the visible behavior identical to the
current one-shot Q&A from the user's side, despite the loop underneath.

## Crate placement

The primitive is product-wide, not chain-specific. The index
maintenance (FTS table, insert hook, startup sweep) belongs with the
database layer (`crates/phoenix-db`); the `MessageRetriever` trait and
`Fts5Retriever` belong beside it or in `crates/phoenix-ide` next to the
other services that compose database + LLM. `chain_qa` becomes a
consumer that holds a `&dyn MessageRetriever`, mirroring how it already
holds a `&dyn LlmService`.

## Failure modes

- **Empty query / no matches:** return an empty result; the consumer
  decides how to degrade (chain Q&A can fall back to the skeleton).
- **Index lag:** a message written (or mutated) but not yet indexed is
  missed by a concurrent query; the same-operation insert/re-index hooks
  and the startup sweep (absent ids + changed fingerprints) keep this
  window minimal, and the freshness watermark lets a surface say "still
  warming" rather than present empty results as authoritative.
  Correctness is eventual against the `messages` truth, never lossy
  (REQ-RET-003).
- **Backend error:** surfaced as `RetrievalError`; the consumer chooses
  whether to fail the Q&A or degrade to skeleton-only context.
