# Phoenix Global Recall MVP

## Goal

Build the MVP foundation for **Phoenix Global Recall**: a global page that gives the user a deterministic overview of active Phoenix work and supports separate read-only Global Recall sessions for cross-conversation analysis.

This is **not** ambient memory for normal coding agents. Normal coding conversations should not receive unrestricted global history tools.

## Product model

Phoenix Global Recall has two coordinated parts:

1. **Global Open Work View**
   - Deterministic backend/UI projection.
   - No LLM required to answer “what active work exists?”
   - Groups by project, then open work item.
   - An open work item is backed by either:
     - a continuation chain, or
     - a standalone conversation.
   - Shows why each item appears using explainable signals.

2. **Global Recall Sessions**
   - Saved read-only LLM analysis sessions under the global page.
   - Multiple sessions may exist at once.
   - Manual new session for a clean slate; no automatic chain summarization in MVP.
   - No managed workspace/files in MVP.
   - Sessions can search/read Phoenix history through host-bound tools and produce strategy answers or handoff reports with citations.

## MVP scope

### Deterministic Global Open Work View

Implement a backend projection and UI surface that lists recent active work grouped as:

```text
Project
  Open work item
    source: chain | conversation
    current/latest conversation
    updated_at
    mode/task/branch metadata when available
    explainable signals
    links/copy reference
```

Candidate signal set:

- Include candidates that are:
  - non-archived
  - user-initiated
  - recently updated
  - grouped so continuation chains appear as one item and old chain members do not clutter the list
- Prioritize items with:
  - Work or Branch mode
  - active/working/blocked/recovery-like state
  - in-progress/ready/blocked task status when task metadata is available
  - very recent activity
  - multi-member chain
- Lower or suppress items with:
  - archived state
  - terminal task status such as done/wont-do
  - old idle activity
  - non-leaf chain members shown separately

Each visible item should expose the reason it appears, e.g. “recent activity,” “Work mode,” “task in progress.”

### References and deep links

Add enough link/reference support for the global page and recall sessions to cite source material.

MVP should prefer app-relative links so deployment hostnames/gateways do not block the feature.

Support at least:

- open conversation/chain from an item
- copy a stable reference handle for an open work item/conversation/chain
- citations in recall answers that link back to source conversations, and ideally messages if message targeting is available in scope

### Global Recall Sessions

Add a global-page-owned saved session concept for read-only cross-conversation analysis.

These sessions should be separate in product behavior from normal coding conversations. They can be stored using existing conversation infrastructure if practical, but they should not appear as ordinary project coding work.

Initial tool model should be host-bound and read-only:

- global message search using the existing `MessageRetriever` with `RetrievalScope::Global`
- paged read of source conversations
- deterministic open-work listing/read access
- reference resolution for copied handles/IDs/links, with natural-language resolution allowed to fall back to search

The agent should produce:

- strategy answers
- handoff reports
- cross-conversation synthesis with source citations

Out of MVP:

- ambient global tools for normal coding agents
- automatic global-session continuation/summarization
- managed filesystem/workspace for global sessions
- task drafting/approval from global sessions
- semantic/vector retrieval

## Implementation notes

Existing foundation:

- `message_fts` already indexes all conversation messages.
- `RetrievalScope::Global` already exists.
- Chain Q&A already has scoped `search_conversations` and `read_conversation` patterns that can inform global read-only tools.
- The global page should build on this substrate rather than adding a parallel message search implementation.

Terminology:

- Use “conversation” for a single conversation.
- Use “chain” for continuation-linked conversations.
- Use “open work item” for the deterministic global view row.
- Avoid introducing “thread” as a product/domain term unless deliberately specified later.

## Acceptance criteria

- A user can open a Global Recall page and see active Phoenix work grouped by project.
- Continuation chains appear as one open work item; standalone conversations appear as one open work item.
- Each item shows deterministic metadata and explainable inclusion/prioritization signals.
- Each item has app-local navigation/deep-link affordances and a copyable reference.
- A user can create multiple saved Global Recall sessions under the global page.
- A Global Recall session can search/read across Phoenix history using host-bound read-only tools.
- Recall answers cite source conversations/messages with links.
- Normal coding agents do not receive global recall tools by default.
- No durable global-agent workspace/files are introduced for MVP.

## Open design details to resolve during implementation

- Exact recency window and sort order for open work items.
- Exact state-machine states that count as active/blocked/recovery-like signals.
- Whether message-level deep links are included in MVP or deferred behind conversation-level links.
- Whether Global Recall sessions are stored as a new table/entity or reuse conversation storage with a product-level discriminator.
- Exact syntax for copied references, e.g. `@work:<id>`, `@chain:<id>`, `@conv:<id>`.

