# ADR-022: The Coordinator uses bounded relational evidence

- **Status:** Accepted
- **Date:** 2026-07-21
- **Supersedes:** ADR-021's decision to retain the inferred open-work projection
- **Affects:** REQ-GR-001–005, REQ-GR-007–011A

## Context

A production briefing combined an inferred open-work capsule, root-based work references, transcript reads, and natural-language message search. The projection knew that a continuation leaf was actively executing but exposed only its root-based work handle; the Coordinator read the older root transcript and treated that historical silence as evidence against the current runtime state. Separately, the projection suppressed another actively executing continuation because its task file was marked done before follow-on work ended.

These failures were not missing special cases alone. They arose because Phoenix classified work and discarded relational identities before the Coordinator interpreted the facts. Correct validation then depended on an optional sequence of reference resolution, current-leaf selection, transcript paging, and search semantics.

## Options considered

1. Patch task/runtime precedence and add current conversation IDs to the capsule.
2. Replace inference with typed filter tools over application-defined activity records.
3. Expose bounded read-only SQLite over operational data, retain bounded transcript tools, and inject one transparent raw snapshot.
4. Expose unrestricted production SQLite, including writes.

## Decision

Phoenix chooses option 3. The Coordinator receives raw current continuation-leaf facts with explicit root/current identities, states, timestamps, and task metadata. It may execute one bounded read-only SQLite statement per `query_database` call. Phoenix does not label rows open, stalled, or attention-worthy.

The Coordinator is trusted for data visibility and may read Phoenix application tables, including hidden messages, credentials, tokens, settings, state, and workflow payloads that may not be visible in normal UI. The SQL integrity and stability boundary is engine-enforced: a separate read-only connection installs a SQLite authorizer and progress handler, denies writes, transactions, pragmas, attached databases, extensions, filesystem functions, and SQLite internal/FTS shadow storage, and applies host-owned SQL, column, row, serialized-output, and execution budgets.

Natural-language `search_conversations`, bounded `read_conversation`, durable `resolve_reference`, and singular `send_conversation_message` remain. Work references continue to resolve root/current identity, but no longer report inferred open/closed status.

## Consequences

- **Positive:** The Coordinator can inspect unforeseen relational questions without waiting for another projection field or heuristic.
- **Positive:** Runtime state, task metadata, chain identity, and transcript evidence remain distinct facts when they disagree.
- **Positive:** Database integrity, filesystem isolation, and resource limits are structural rather than prompt-based.
- **Positive:** New Phoenix application tables become readable without maintaining a confidentiality allowlist.
- **Negative:** The Coordinator is coupled to the application database schema and must formulate SQL.
- **Negative:** Raw operational rows may cost more tokens and require explicit interpretation.
- **Negative:** SQLite expressiveness creates availability risk, mitigated by connection authority, authorization, progress interruption, and output bounds.
- **Neutral:** Historical transcript search and citations retain their existing specialized interfaces.

## References

- `specs/global-recall/requirements.md`
- `Database::coordinator_query`
- `GlobalReadService::coordinator_snapshot`
- `query_database`
