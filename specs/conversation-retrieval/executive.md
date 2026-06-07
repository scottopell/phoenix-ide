# Conversation Retrieval — Executive Summary

## Requirements Summary

Conversation Retrieval is a single, scope-filtered retrieval primitive
over the content of every conversation Phoenix has recorded. Given a
natural-language query and a **scope** — a set of conversation ids, or
"everything" — it returns the most relevant message chunks with their
provenance (which conversation, which message, when, what role). The
primitive has exactly one index over all messages; callers differ only
in the scope they pass. Chain Q&A passes the chain's member ids;
a future application-wide Q&A passes the global scope. Both read from
the same index through the same call.

The first consumer is a read-only **agentic** Q&A loop (chains'
REQ-CHN-009): the primitive is exposed to that agent as a search tool,
alongside a scope-bound read-content tool, so the agent can search,
read full conversations, and iterate within its scope. The scope is
bound by the host at tool-construction time, never chosen by the model,
so the same agent becomes chain Q&A or global Q&A purely by which scope
it is handed — and cannot escape it.

This primitive exists so that recall over conversation content scales
with the **specificity of the question**, not with the size of the
corpus, and so that every recall surface in Phoenix is built on one
substrate rather than each re-deriving its own context-assembly
strategy.

## Technical Summary

The index is an SQLite FTS5 virtual table populated from message text
extracted from the typed `MessageContent` of each message (not raw
JSON), with `conversation_id`, `message_id`, `message_type`, and
`created_at` carried as unindexed columns so scope filtering happens
inside the query. The index is a **derived cache** over the `messages`
table (the source of truth): it is kept current by indexing on message
persist and reconciled by an idempotent startup backfill, and it can be
rebuilt from `messages` at any time. The FTS5 table and its backfill
are introduced through a migration.

Retrieval is exposed behind a `MessageRetriever` trait. The first
backend is FTS5/BM25 (lexical), chosen because the bundled SQLite ships
FTS5 — no new dependency, no embedding provider, no network on the
query path. The trait is the seam: a vector backend (embeddings +
similarity) or a hybrid backend (rank-fused lexical + vector) drops in
behind it without changing any caller. Scope is an enum
(`Conversations(set)` / `Global`, with `Project` and `Since` as named
extension points), applied as a query predicate rather than a
post-filter so `top_k` is honored after scoping.

Chain Q&A consumes this primitive in place of bundling per-member
continuation summaries: the chain's members are resolved to ids, the
retriever returns the chunks relevant to the question across those
members, and those chunks — with a lightweight chain skeleton for
orientation — form the model context. Because retrieval runs against
the live index, answers reflect the chain's current state by
construction.

## Status Summary

This is a design specification. No requirement below is implemented
yet; this table is the implementation tracker.

| Requirement | Status | Notes |
|---|---|---|
| **REQ-RET-001:** Scope-Filtered Retrieval Primitive | Planned | |
| **REQ-RET-002:** One Index Over All Conversation Messages | Planned | |
| **REQ-RET-003:** Index Is a Rebuildable Derived Cache | Planned | Migration-introduced FTS5 table + backfill |
| **REQ-RET-004:** Text Extracted From Typed Message Content | Planned | Shared extraction with chain-qa transcript rendering |
| **REQ-RET-005:** Retrieval Backend Is Swappable | Planned | FTS5/BM25 first; vector/hybrid behind the trait |
| **REQ-RET-006:** Results Carry Provenance | Planned | |
| **REQ-RET-007:** Scope Is Applied In-Query, Not Post-Hoc | Planned | |
| **REQ-RET-008:** Scope Is Host-Bound When Retrieval Is a Tool | Planned | Agent supplies query only; host fixes scope at tool construction |

## Scope

The MVP ships REQ-RET-001 through REQ-RET-007 with the FTS5/BM25
backend and one consumer (chain Q&A, `specs/chains/` REQ-CHN-009). The
application-wide Q&A surface that motivates the `Global` scope is a
separate spec; this primitive is built to serve it but does not depend
on it.

## Out of Scope (Tracked for Future)

- **Vector / hybrid retrieval backend.** The `MessageRetriever` seam
  exists for it; the MVP does not implement it. Trigger to pivot:
  lexical recall proves insufficient for paraphrase-heavy questions, or
  a product decision to add semantic ambient memory. A vector backend
  additionally requires an embedding provider (the Anthropic provider
  offers none) and a chunk-splitting strategy for long messages.
- **Application-wide Q&A surface.** The `Global` scope is the substrate;
  the surface that uses it (and the dashboard it lives on) is specified
  elsewhere.
- **Re-ranking or query rewriting inside the primitive.** One `retrieve`
  call is one ranked lookup; the consuming agent supplies iteration by
  calling it repeatedly (REQ-RET-008).
- **Indexing of non-message artifacts** (task files, PR feedback, diffs).
  The index covers conversation messages only.
- **Per-user or cross-database federation.** One database, one index.
