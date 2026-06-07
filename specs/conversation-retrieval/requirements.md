# Conversation Retrieval

## User Story

As a Phoenix user, I ask recall questions about work that happened
across my conversations — "what did we decide about the auth schema?",
"which conversation touched the rate limiter?", "where did we leave the
migration?". I want the answer to come from the **actual content** of
the relevant conversations, found by relevance to my question, not from
pre-baked summaries that flatten everything to the same resolution
regardless of what I asked. I want the same recall to work whether I am
asking within one chain of related conversations or across everything I
have ever run.

## Why the User Cares

- **Specificity beats summary.** A summary answers the questions its
  author anticipated. Retrieval answers the question actually asked, by
  surfacing the messages that bear on it.
- **One substrate, every surface.** Recall inside a chain and recall
  across the whole product are the same operation with a different
  scope. A user should not get better answers in one place than another
  because two surfaces assembled context differently.
- **Cost tracks the question, not the corpus.** Bundling every member's
  content grows with the corpus. Retrieval grows with how specific the
  question is, so a pointed question stays cheap even over a large
  history.

## Transparency Contract

The user (and any consuming surface) must be able to answer:

1. Which conversation, and which message in it, did this answer draw
   from?
2. Was the retrieval restricted to a set of conversations (a chain), or
   run across everything?
3. Is the index current with the conversations it claims to cover?

## Requirements

### REQ-RET-001: Scope-Filtered Retrieval Primitive

WHEN a caller requests retrieval with a query string, a scope, and a
result limit `top_k`
THE SYSTEM SHALL return up to `top_k` message chunks drawn only from
conversations admitted by the scope, ranked by relevance to the query

THE scope SHALL be one of:
- `Conversations(ids)` — retrieval is restricted to messages whose
  conversation id is in the given set (the chain case)
- `Global` — retrieval spans every conversation in the database (the
  application-wide case)

THE retrieval primitive SHALL be the single entry point for all
conversation-content recall surfaces; a surface SHALL NOT assemble
recall context by re-querying the `messages` table directly.

**Rationale:** The scope parameter is the *only* axis that separates
chain recall from application-wide recall. Making it a parameter of one
primitive — rather than two parallel code paths — is what guarantees
both surfaces answer with the same quality. Routing all recall through
this entry point keeps context-assembly strategy in one place that can
be improved (e.g. swapping the backend) once for everyone.

---

### REQ-RET-002: One Index Over All Conversation Messages

THE SYSTEM SHALL maintain exactly one retrieval index covering the
messages of every conversation, regardless of which surface will query
it

THE index SHALL carry, for each indexed chunk, the chunk's
`conversation_id`, `message_id`, `message_type`, and `created_at`, so
that scope filtering (REQ-RET-007) and provenance (REQ-RET-006) are
satisfiable from the index alone

THE SYSTEM SHALL NOT maintain a separate per-chain or per-surface index

**Rationale:** A chain is a subset of all conversations; the global
corpus is the superset. Indexing the superset once and filtering down
is strictly simpler than maintaining overlapping per-scope indexes that
must be kept mutually consistent. Carrying the scope/provenance keys in
the index avoids a second round-trip to `messages` at query time.

---

### REQ-RET-003: Index Is a Rebuildable Derived Cache

THE retrieval index SHALL be a derived view of the `messages` table,
which remains the sole source of truth for conversation content

WHEN a message is persisted
THE SYSTEM SHALL index it so that it becomes retrievable without
requiring a restart

WHEN the server starts
THE SYSTEM SHALL reconcile the index against `messages` and index any
message that is present in `messages` but absent from the index, idempotently

THE SYSTEM SHALL be able to rebuild the entire index from `messages`
with no loss of source data

THE index structure and its backfill SHALL be introduced through a
database migration, not through serde defaults or ad-hoc runtime table
creation

**Rationale:** Treating the index as a cache over `messages` means a
corrupt, stale, or schema-changed index is never a data-loss event — it
is rebuilt. Insert-time indexing keeps live conversations searchable;
the startup reconciliation closes the gap for messages written while a
prior index shape was in effect or before the index existed. The
migration requirement follows the repo-wide rule that persisted
structure changes are owned by migrations.

---

### REQ-RET-004: Text Extracted From Typed Message Content

WHEN indexing a message
THE SYSTEM SHALL extract human-readable text from the message's typed
`MessageContent`, not index the raw stored JSON

THE extraction SHALL cover every `MessageContent` variant; a variant
whose body is not natural-language prose (e.g. a tool result) SHALL be
reduced to a compact, low-noise representation rather than indexed
verbatim or silently dropped

THE text extraction used for indexing SHALL be the same logic used to
render conversation transcripts for model context, so the indexed text
and the text a model would read cannot diverge

**Rationale:** Indexing raw JSON pollutes the index with structural
tokens (`"role"`, `"type"`, braces) that match queries spuriously and
crowd out content. Tool results are mostly machine output; indexing
them verbatim drowns the signal, but dropping them silently is data
loss indistinguishable from a bug — a compact marker is the correct
middle. Sharing one extraction routine with transcript rendering means
the index and the model see the same words.

---

### REQ-RET-005: Retrieval Backend Is Swappable

THE retrieval primitive SHALL expose its query operation behind an
interface that hides the ranking backend from callers

THE first backend SHALL be lexical (FTS5/BM25), requiring no embedding
provider and no network call on the query path

WHEN a different ranking backend is introduced (vector similarity, or a
rank-fused hybrid)
THE SYSTEM SHALL be able to substitute it behind the same interface
without changing any caller

**Rationale:** Lexical retrieval is the honest MVP: the bundled SQLite
ships FTS5, so it adds no dependency, runs offline, and answers the
lexical recall questions that dominate ("which file", "the auth bug",
"what optimizations"). The interface seam is what lets a semantic
backend land later as a pure substitution — callers, having only ever
seen `retrieve(query, scope, top_k)`, are unaffected. A vector backend
is deferred because it requires an embedding provider (the Anthropic
provider offers none) and a chunk-splitting strategy; neither belongs
in the MVP.

---

### REQ-RET-006: Results Carry Provenance

WHEN the retrieval primitive returns a chunk
THE SYSTEM SHALL include the chunk's source `conversation_id`,
`message_id`, `message_type`, `created_at`, a relevance score, and a
text snippet suitable for display or for assembly into model context

**Rationale:** A retrieved chunk with no provenance cannot be cited,
attributed, or ordered chronologically when assembled into a prompt,
and a consuming UI cannot link the answer back to its source
conversation. Provenance is what makes a retrieved answer auditable
rather than an oracle.

---

### REQ-RET-007: Scope Is Applied In-Query, Not Post-Hoc

WHEN a scope restricts retrieval to a set of conversations
THE SYSTEM SHALL apply the restriction as part of the ranked query, such
that `top_k` returns the top results **within the scope**

THE SYSTEM SHALL NOT retrieve a global top_k and then discard
out-of-scope results, which would yield fewer than `top_k` in-scope
results (or none) for a narrow scope over a large corpus

**Rationale:** Post-hoc filtering silently starves narrow scopes: a
chain of three conversations inside a corpus of thousands would have
its results crowded out of a global top_k before the filter runs,
returning a near-empty set. Pushing the scope predicate into the ranked
query guarantees the chain gets its own top_k.

---

### REQ-RET-008: Scope Is Host-Bound When Retrieval Is a Tool

WHEN the retrieval primitive (or a companion read-content capability) is
exposed to an LLM as a callable tool — for example to a read-only Q&A
agent
THE SYSTEM SHALL bind the scope at the host when the tool is
constructed, so the model supplies only the query (and, for read, a
target conversation/message within scope)
THE model SHALL NOT be able to specify, widen, or override the scope
through tool arguments

WHEN a tool call names a conversation outside the bound scope (e.g. a
read for a conversation not in the bound set)
THE SYSTEM SHALL refuse it rather than serving out-of-scope content

**Rationale:** An agentic consumer (chains' read-only Q&A, the future
application-wide Q&A) drives retrieval as a tool and iterates. If the
model could choose its own scope, a chain Q&A agent could read
conversations outside the chain — the scope would be a suggestion, not a
boundary. Making the host fix the scope at tool-construction time makes
the boundary structural: the same agent code becomes chain Q&A or global
Q&A purely by which scope the host binds, and neither can escape its
binding. This is the correct-by-construction form of "the model can dig
as deep as it wants, but only within the conversations it was given."

---

## Non-Requirements (explicit out-of-scope for the MVP)

- **Vector or hybrid ranking.** The seam (REQ-RET-005) is required; the
  semantic backend behind it is not part of the MVP.
- **Chunk splitting of long messages.** The MVP indexes one chunk per
  message. Sub-message chunking matters for an embedding backend and is
  deferred with it.
- **Re-ranking, query rewriting, or multi-hop logic inside the
  primitive.** A single `retrieve` call does one ranked lookup. Iteration
  (search again, read more, refine) is the consuming agent's job
  (REQ-RET-008 / chains REQ-CHN-009), achieved by calling the primitive
  repeatedly — not built into the primitive.
- **Indexing artifacts other than conversation messages** (task files,
  diffs, PR feedback). Out of scope for this index.
- **Relevance feedback / learning from which answers the user kept.**
  No requirement defines it.

## Future Direction (named, not MVP)

- **Vector / hybrid backend behind the existing seam.** Per-message (or
  per-chunk) embeddings plus similarity search, optionally rank-fused
  with BM25 (reciprocal-rank fusion). Substituted behind REQ-RET-005's
  interface. Requires an embedding provider and a chunk-splitting
  strategy. **Trigger to pivot:** paraphrase-heavy questions recall
  poorly under pure lexical ranking, or a product decision to add
  semantic ambient memory across conversations.
- **Additional scopes.** `Project(id)` and `Since(timestamp)` extend
  the scope enum for surfaces that want "recall within this project" or
  "recall recent activity"; they apply as additional in-query
  predicates (REQ-RET-007) with no change to the primitive's shape.
