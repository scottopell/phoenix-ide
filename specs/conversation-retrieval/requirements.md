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

THE query SHALL be accepted as **natural language** (a user's question),
and the primitive SHALL build whatever backend query that requires so
that relevant messages are returned without demanding that every word of
the question appear. A naive pass of the whole question to the backend —
which for a lexical backend means an implicit AND across every filler
word, or an exact-phrase match if the whole string is quoted — would make
ordinary recall questions return nothing; the primitive owns the
query-construction that prevents this (see design), so callers pass plain
questions.

THE retrieval primitive SHALL be the single entry point for **ranked
relevance recall** of conversation content; a surface SHALL NOT roll its
own relevance/ranking strategy by re-querying the `messages` table
directly.

THIS prohibition governs ranked recall only. Reading the **full content
of an already-identified conversation** (the scope-bound read capability
of REQ-RET-008, used by an agent after retrieval has pointed it at a
conversation) is a distinct, non-ranked operation and is permitted —
it, too, is scope-bound, but it is not a parallel retrieval strategy.

**Rationale:** The scope parameter is the *only* axis that separates
chain recall from application-wide recall. Making it a parameter of one
primitive — rather than two parallel code paths — is what guarantees
both surfaces answer with the same quality. Routing all *ranking*
through this entry point keeps relevance strategy in one place that can
be improved (e.g. swapping the backend) once for everyone. Fetching a
known conversation's content is not ranking and does not compete with
that strategy, so it is not what this prohibition forbids.

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

WHEN an existing message's stored content is mutated (message content is
not strictly append-only — e.g. a tool message's content is updated in
place after the tool completes)
THE SYSTEM SHALL re-index that message so its indexed text reflects the
new content, rather than leaving the prior extraction in place

WHEN a message is deleted from `messages` (including the cascade when a
conversation is hard-deleted)
THE SYSTEM SHALL remove that message's index rows, so deleted content is
never returned by retrieval. The index SHALL NOT outlive its source: a
row whose source `message_id` no longer exists in `messages` is a defect
(deleted content resurfacing in recall), not a stale-but-harmless cache
entry.

WHEN the server starts
THE SYSTEM SHALL reconcile the index against `messages` in both
directions: index any message present in `messages` but absent from the
index; re-index any message whose stored content has changed since it was
last indexed (detected by a content fingerprint or version, not by
id-presence alone); AND prune any index row whose source `message_id` is
no longer present in `messages` (repairing deletions that occurred while
a delete hook was absent or a prior index shape was in effect),
idempotently

THE SYSTEM SHALL be able to rebuild the entire index from `messages`
with no loss of source data

THE SYSTEM SHALL be able to report the index's **freshness** — at
minimum, whether startup reconciliation has completed and whether any
message in `messages` is newer than the index has caught up to — so that
a consuming surface (Transparency Contract item 3) can distinguish "no
in-scope content matched" from "the index has not caught up yet." Until
reconciliation completes, a surface MAY indicate that recall is still
warming rather than presenting empty results as authoritative.

THE index table SHALL be created by a database migration (not by serde
defaults or ad-hoc runtime table creation). The migration creates the
empty index structure; the **typed text backfill** that populates it
from existing `messages` is performed by the Rust startup reconciliation
above, because the indexed text is a Rust-side extraction of typed
`MessageContent` (REQ-RET-004) that static SQL cannot reproduce from the
stored JSON.

**Rationale:** Treating the index as a cache over `messages` means a
corrupt, stale, or schema-changed index is never a data-loss event — it
is rebuilt. Insert-time indexing keeps live conversations searchable;
re-indexing on content mutation keeps the cache honest when a row
changes after first index (indexing only newly-absent ids would let an
updated tool result keep its stale extraction, and the index would no
longer be a current view of `messages`); deleting index
rows when their source disappears is a correctness requirement, not
optional cleanup: a standalone FTS table has no foreign key to cascade
the delete, so an orphaned row would resurface hard-deleted content in
recall. The startup reconciliation closes all three gaps — absent ids,
changed rows, and orphaned rows — for messages written, mutated, or
deleted while a prior index shape was in effect or before the index
existed. Splitting table-creation (migration) from text-backfill (Rust)
respects both the repo-wide "persisted structure changes are owned by
migrations" rule and the reality that the static-SQL migration framework
cannot run typed extraction.

---

### REQ-RET-004: Text Extracted From Typed Message Content

WHEN indexing a message
THE SYSTEM SHALL extract human-readable text from the message's typed
`MessageContent`, not index the raw stored JSON

THE extraction SHALL cover every `MessageContent` variant. A variant
whose body is machine output (e.g. a tool result) SHALL be reduced to a
**bounded searchable excerpt** — size-capped, but capturing the parts of
the output most likely to carry the searchable signal, which for command
output and build logs is **both the head and the tail** (an error,
failing test name, or file path is commonly on the last lines, not the
first). A head-only truncation is insufficient. The excerpt SHALL NOT be
indexed verbatim-unbounded, and SHALL NOT be silently dropped to a
content-free marker. Where even head+tail would miss mid-body terms, the
content-bearing read path (REQ-RET-008) recovers them once search has
located the conversation — but the index SHALL carry enough that a
tail-only error term still surfaces the conversation in the first place.

THE text extraction used for indexing SHALL be the same logic used to
render the compact conversation transcript for model orientation, so the
indexed text and that transcript cannot diverge

THE size-capping of tool results is a property of the
**index/orientation** text only. The scope-bound **read capability**
(REQ-RET-008) that an agent uses to inspect an identified conversation
SHALL be able to return the **full** content of a message, including the
complete body of a tool result — capping is a ranking-signal optimization
for the index, not a ceiling on what can be read. (The read capability
bounds size by paging, REQ-RET-008, not by truncation.)

**Rationale:** Indexing raw JSON pollutes the index with structural
tokens (`"role"`, `"type"`, braces) that match queries spuriously and
crowd out content. Tool results are machine output, but they are
*content* — the answer to a recall question is often a term that appears
only in a build log, grep output, or a file path. Indexing them
unbounded would let one giant result dominate the FTS table; indexing
only a `(tool result: N chars)` marker would make those terms
unretrievable (and the full read tool cannot help until *after* search
has found the conversation). A size-capped excerpt is the correct
middle: searchable leading content, bounded cost. Reading is different
from ranking: once an agent
has decided a conversation is relevant, it must be able to read the
actual tool output, build log, or sub-agent result that holds the
answer, so the read path returns full content (paged) rather than the
folded marker. Sharing one extraction routine between the index and the
orientation transcript keeps those two in lockstep; the read path is the
deliberate, separately-typed full-content path.

---

### REQ-RET-005: Retrieval Backend Is Swappable

THE retrieval primitive SHALL expose its query operation behind an
interface that hides the ranking backend from callers

THE ranking backend SHALL be lexical (FTS5/BM25), requiring no embedding
provider and no network call on the query path

WHEN a different ranking backend is substituted (vector similarity, or a
rank-fused hybrid)
THE SYSTEM SHALL be able to substitute it behind the same interface
without changing any caller

**Rationale:** Lexical ranking is the honest default: the bundled SQLite
ships FTS5, so it adds no dependency, runs offline, and answers the
lexical recall questions that dominate ("which file", "the auth bug",
"what optimizations"). The interface seam is what lets a semantic
backend be substituted as a pure swap — callers, having only ever seen
`retrieve(query, scope, top_k)`, are unaffected. A vector backend sits
outside this contract because it requires an embedding provider (the
Anthropic provider offers none) and a chunk-splitting strategy, neither
of which this spec defines.

---

### REQ-RET-006: Results Carry Provenance

WHEN the retrieval primitive returns a chunk
THE SYSTEM SHALL include the chunk's source `conversation_id`,
`message_id`, a **chunk identity** that locates the chunk **within** its
message, `message_type`, `created_at`, a relevance score, and a text
snippet suitable for display or for assembly into model context

THE chunk identity SHALL distinguish two chunks that originate from the
same `message_id` (a backend that splits a long message into multiple
chunks SHALL assign each a stable ordinal and/or character range), so a
caller can cite, de-duplicate, and re-read a specific chunk. For a
backend that indexes one chunk per message, the identity is the whole
message (ordinal 0 / full range); the field is part of the result shape
unconditionally so that shape does not change when a chunking backend is
substituted.

**Rationale:** A retrieved chunk with no provenance cannot be cited,
attributed, or ordered chronologically when assembled into a prompt,
and a consuming UI cannot link the answer back to its source
conversation. `message_id` alone is insufficient the moment a backend
splits a message: two chunks from one message become indistinguishable,
so a caller cannot cite *which part*, cannot de-duplicate, and cannot
re-read the exact span. Because REQ-RET-005 promises a chunking
vector/hybrid backend can be substituted **without changing callers**,
the chunk identity must be in the result shape unconditionally — were it
added only alongside a chunking backend, that would be the
caller-visible change the seam is supposed to prevent. Provenance is what
makes a retrieved answer auditable rather than an oracle.

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
THE SYSTEM SHALL fix the scope at the host when the tool is
constructed, so the model supplies only the query (and, for read, a
target conversation/message within scope)
THE model SHALL NOT be able to specify, widen, or override the scope
through tool arguments

THE host-fixed scope MAY be a **rule resolved live** rather than a
frozen list — e.g. "the members of chain root X," re-resolved at each
tool call — so that membership changes during a run are reflected. What
is fixed at construction is the *boundary the model cannot cross*, not
necessarily a static set; liveness within that boundary is permitted and,
for chain Q&A, required (see chains REQ-CHN-009).

WHEN a tool call names a conversation outside the bound scope (e.g. a
read for a conversation not in the bound set)
THE SYSTEM SHALL refuse it rather than serving out-of-scope content

WHEN the read-content tool is asked for a conversation whose full
content would not safely fit a single tool result (chain members can be
large enough to have triggered context-exhaustion continuation in the
first place)
THE read tool SHALL expose a **bounded/paged** contract — a cursor plus
a size limit, with an indication that more remains — rather than
returning an unbounded full transcript in one call
THE size limit SHALL be **enforced by the host**, not merely defaulted: a
model-supplied size argument is clamped to a fixed host ceiling
(`min(requested, cap)`), so the model cannot request a page larger than
the next call can safely hold
THE bound SHALL be by size (bytes/tokens), not message count, and SHALL
hold within a single oversized message (continuation by intra-message
offset), so one huge message cannot exceed the bound
THE SYSTEM SHALL NOT return a single read result large enough to push
the next bounded-loop model call past its context window

**Rationale:** An agentic consumer (chains' read-only Q&A, or an
application-wide Q&A surface) drives retrieval as a tool and iterates. If the
model could choose its own scope, a chain Q&A agent could read
conversations outside the chain — the scope would be a suggestion, not a
boundary. Making the host fix the scope at tool-construction time makes
the boundary structural: the same agent code becomes chain Q&A or global
Q&A purely by which scope the host binds, and neither can escape its
binding. This is the correct-by-construction form of "the model can dig
as deep as it wants, but only within the conversations it was given."

---

## Non-Requirements (out of scope)

These are capabilities this spec deliberately does not define. The
`MessageRetriever` seam (REQ-RET-005) and the scope enum are shaped so
each could be added behind them without changing callers; what is *not*
defined here is the capability itself. (Sequencing — what is built
first — lives in `executive.md`, not in these standing requirements.)

- **A semantic (vector) or hybrid ranking backend.** This spec defines
  the lexical ranking contract and the seam that admits another backend;
  it does not define embedding generation, vector storage, or rank
  fusion. A semantic backend additionally depends on an embedding
  provider (the Anthropic provider offers none) and a sub-message
  chunk-splitting strategy, neither of which this spec specifies.
- **Sub-message chunk splitting.** The index carries one chunk per
  message; splitting a long message into multiple chunks is a property of
  a chunking backend, not of this contract (the chunk-identity result
  shape, REQ-RET-006, already admits it).
- **Re-ranking, query rewriting beyond the natural-language handling of
  REQ-RET-001, or multi-hop logic inside the primitive.** A single
  `retrieve` call does one ranked lookup. Iteration (search again, read
  more, refine) is the consuming agent's job (REQ-RET-008 / chains
  REQ-CHN-009), achieved by calling the primitive repeatedly.
- **Indexing artifacts other than conversation messages** (task files,
  diffs, PR feedback). This index covers conversation messages only.
- **Relevance feedback / learning from which answers the user kept.**
  Not defined.
- **Scopes beyond `Conversations` and `Global`.** `Project(id)` and
  `Since(timestamp)` are natural extensions of the scope enum — they
  would apply as additional in-query predicates (REQ-RET-007) with no
  change to the primitive's shape — but this spec defines only the two
  scopes its consumers require.
