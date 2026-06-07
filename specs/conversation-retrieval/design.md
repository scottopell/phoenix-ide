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
    pub message_type: MessageType,
    pub created_at: DateTime<Utc>,
    pub snippet: String,
    pub score: f64,
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
    text,                       -- extracted, searchable
    message_id     UNINDEXED,
    conversation_id UNINDEXED,
    message_type   UNINDEXED,
    created_at     UNINDEXED
);
```

`UNINDEXED` columns are stored but not tokenized: they are available to
the `WHERE`/projection without participating in the match, which is
exactly what scope filtering (REQ-RET-007) and provenance
(REQ-RET-006) need. Carrying them on the FTS row removes any join back
to `messages` at query time.

The table and the backfill that populates it from existing `messages`
ship in one migration (REQ-RET-003), alongside the other migrations in
`crates/phoenix-db`. The table is keyed for rebuild: a full rebuild is
`DELETE FROM message_fts` followed by re-extraction from `messages`.

### Keeping the index current

- **On persist.** The message write path extracts text and inserts the
  FTS row in the same logical operation that writes the message, so a
  freshly persisted message is immediately retrievable
  (REQ-RET-003). Insertion is `INSERT` guarded so a re-run for the same
  `message_id` does not duplicate a row (delete-then-insert on
  `message_id`, or an existence check).
- **On startup.** A reconciliation sweep indexes any `message_id`
  present in `messages` but absent from `message_fts`. This is the
  backfill for pre-existing messages and the repair path for any window
  where indexing lagged. It is idempotent and safe to run every boot.

Triggers on `messages` are deliberately *not* used to maintain the FTS
table, because the indexed text is a Rust-side extraction of typed
`MessageContent` (REQ-RET-004) that SQL triggers cannot reproduce from
the JSON column. The Rust write path owns extraction; the startup sweep
owns reconciliation.

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
| `Tool` | compact marker (`(tool result: N chars)`) — folded, not verbatim, not dropped |

Folding tool results keeps machine output from drowning the index while
leaving a visible, countable trace (REQ-RET-004 rationale). If lexical
recall over tool output proves valuable later, the marker can be
widened to a truncated head without changing the index shape.

## The MVP backend: `Fts5Retriever` (REQ-RET-005)

```sql
-- scope = Conversations([a, b, c])
SELECT text, message_id, conversation_id, message_type, created_at,
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
thinned by a post-filter. `snippet(message_fts, …)` provides the
display snippet; `bm25()` provides the score (REQ-RET-006).

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
  -> retriever.retrieve(query, BOUND_SCOPE, top_k)   // ranked chunks

read_conversation { conversation_id }                // must be in BOUND_SCOPE
  -> full extracted transcript of that member
```

`BOUND_SCOPE` is fixed when the tools are constructed (REQ-RET-008): for
chain Q&A it is `Conversations(chain_members_forward(root))`; for a
future application-wide Q&A it is `Global`. The model never sees a scope
argument, so it cannot widen its reach. `read_conversation` validates
its `conversation_id` against the bound scope and refuses anything
outside it (for `Global`, any conversation is in scope).

`read_conversation` returns extracted transcript text via the same
extraction as indexing (REQ-RET-004), so what the agent reads matches
what was indexed and what a transcript renderer would show — no parallel
representation.

## Consumer: chain Q&A loop

`specs/chains/` REQ-CHN-009 specifies the chain side. The Q&A run:

1. Resolve members with `Database::chain_members_forward(root_id)`.
2. Construct `search_conversations` + `read_conversation` bound to
   `Conversations(member_ids)`.
3. Run a read-only agent (no bash, no patch, no worktree) seeded with
   the question and a cheap **chain skeleton** (member titles + trailing
   continuation summaries) for orientation. The agent iterates:
   search → read promising members in full → search again → answer.
4. Stream the answer over the existing chain-scoped SSE broadcaster.

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

### Reusing the existing agent runtime vs. a bespoke loop

Phoenix already has an agentic runtime (state machine + tool executor +
SSE streaming + sub-agent plumbing). Two implementation options:

- **Reuse the runtime** as a constrained, ephemeral, read-only
  conversation whose toolset is just the two scope-bound tools. Gets
  streaming, persistence, and tool execution for free; pays the cost of
  bending the conversation lifecycle to an ephemeral Q&A that persists
  to `chain_qa`, not to `conversations`.
- **Bespoke bounded loop** in the Q&A module: a small search/read/answer
  loop calling `LlmService::complete` with the two tools and a turn cap.
  Simpler lifecycle, no worktree/state-machine baggage; reimplements a
  thin slice of the tool-execution loop.

This is the load-bearing implementation decision for REQ-CHN-009 and is
called out for resolution before build. The retrieval primitive and the
two tools are identical either way.

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
- **Index lag:** a message written but not yet indexed is missed by a
  concurrent query; the startup sweep and same-operation insert keep
  this window minimal. Correctness is eventual against the `messages`
  truth, never lossy (REQ-RET-003).
- **Backend error:** surfaced as `RetrievalError`; the consumer chooses
  whether to fail the Q&A or degrade to skeleton-only context.
