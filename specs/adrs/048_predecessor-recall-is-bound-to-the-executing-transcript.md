# ADR-048: Predecessor recall is bound to the executing transcript

- **Status:** Accepted
- **Date:** 2026-09-12
- **Affects:** REQ-RET-009, REQ-GR-007

## Context

ADR-027 gives ordinary write-capable parents global evidence tools, including
conversation search and reads. That answers whether an agent can retrieve old
work, but does not make its own pre-compaction evidence directly discoverable.
The user wants looking back through predecessor transcripts to be a natural
continuation path. Restricted planning parents need that same continuity
without receiving global database, history, or messaging authority.

The ProductConversation program also describes whole-conversation and typed
source retrieval. Neither relation alone identifies the historical prefix
before the executing transcript. Source-linked follow-ups are separate work;
an aggregate's latest member can change after a runtime was constructed.

## Options considered

1. **Prompt advice over global tools.** Small change, but global results can
   obscure predecessor evidence and restricted planners lack those tools.
2. **A model-selected scope on global search.** Convenient reuse, but makes the
   predecessor boundary optional and complicates eligibility for planners.
3. **A host-bound predecessor capability.** Adds a discoverable tool boundary
   while reusing ranked retrieval and bounded source reads. The host owns
   identity, membership validation, and the exclusion of unrelated work.
4. **Inject all history or depend entirely on the compaction summary.** Either
   grows context with history or makes omitted detail inaccessible in practice.

## Decision

Choose option 3. Expose one `previous_transcripts` tool with a closed operation
set: `list`, `search`, and `read`. Bind the ordinary ProductConversation and
executing parent transcript at construction. Resolve the strict predecessor
prefix from authoritative membership and continuation topology on every call;
never replace the bound transcript with the aggregate's latest member.

The model selects an operation, a natural-language query for search, or a typed
transcript reference for read. Listing and reading accept optional paging cursors.
It supplies no aggregate, root, workspace, source, or scope selector. Model
arguments cannot alter the host binding. Cursors remain target-bound and cannot
authorize a read outside that binding. Listing cursors are bound to the executing
transcript's predecessor scope; read cursors additionally bind the source target.
Represent valid operation inputs and
outcomes with closed types rather than loosely combined optional fields.

All ordinary parent registries receive the capability, including restricted
planning registries. Global evidence eligibility remains governed by ADR-027.
Subagents and the Coordinator do not receive this tool. A follow-up's typed
source is not a predecessor; source recall retains its independent contract.

Use `MessageRetriever` with `RetrievalScope::Conversations` for ranked search.
Reuse bounded source-content reading and provenance behavior. Add no index,
transcript store, or mutable aggregate root/latest cache. Listing and live reads
remain available when the derived retrieval index cannot establish coverage.

Bounded host-authored orientation names the immediate predecessor and explains
the tool. The agent can list additional references or read known predecessors
without a search hit. Orientation carries references and guidance, not ambient
transcript bodies. Continuation and runtime reconstruction use the same
authoritative orientation source, respecting provider prompt projection.

## Consequences

- **Positive:** The same-work recall path is obvious even to restricted
  planners, without granting unrelated global capabilities.
- **Positive:** Fixed executing-member identity prevents later continuation
  activity from silently expanding an older runtime's view into its successors.
- **Positive:** Existing retrieval and full-content paging remain the shared
  foundation; search coverage does not determine whether source text exists.
- **Negative:** Runtime registration, prompt orientation, and target-bound
  paging need a dedicated capability boundary and regression coverage.
- **Negative:** Write-capable parents have both global and predecessor tools;
  tool guidance must explain when the narrower operation is useful.
- **Neutral:** This decision does not grant follow-up source access, change
  lifecycle policy, or replace whole-conversation user-facing Q&A.

## References

- ADR-027, ADR-031, ADR-045, ADR-046
- `specs/conversation-retrieval/requirements.md` REQ-RET-009
- `specs/global-recall/requirements.md` REQ-GR-007
- `specs/chains/requirements.md` REQ-CHN-009
- `coordinator_tools::writing_tools`, `GlobalReadService`
- `MessageRetriever`, `RetrievalScope`, `ToolRegistryExecutor`
