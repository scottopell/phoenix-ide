# Phoenix Coordinator — Executive Summary

## Requirements Summary

Phoenix Coordinator is one durable, chat-first Phoenix-wide conversation for surveying unrelated work, inspecting relevant history, and sending useful text guidance to existing conversations. The Coordinator obtains fresh relational facts on demand through `query_database`; ordinary requests contain no automatically injected activity snapshot. Both the Coordinator and write-capable ordinary ProductConversations receive bounded natural-language history search, conversation reading, read-only operational SQLite, and singular cross-conversation messaging; stable reference resolution remains Coordinator-only.

The Coordinator has two host capabilities that can perform mutations: singular text-message delivery to an existing non-Coordinator conversation, and unsandboxed Bash targeted to an explicit active WorkScope. A Coordinator-only built-in skill documents supported Phoenix HTTP APIs for user-authorized lifecycle actions through scoped Bash. Those calls preserve normal API authorization and require response plus resulting-state verification. Message delivery reuses the normal chat acceptance authority, so each target independently reports delivered, queued as steering, or rejected. Acceptance never implies that the receiving agent understood, acknowledged, executed, or completed the instruction.

The `/global` surface keeps the standard transcript and composer without a separate work view. Bounded subordinate surfaces expose only the current server-backed watch subscriptions and still-running Coordinator-launched Bash commands; both use authoritative identities and navigation, and the Bash surface remains process-local. A compact composer action requests a read-only current-activity briefing through the normal message path while preserving the user's draft.

## Technical Summary

Coordinator LLM requests contain the stable language-specific system prompt without an automatic current-activity capsule. The Brief me composer action requests a fresh, bounded, read-only check-in focused on decisions and blockers needing attention, followed by progressing work. The Coordinator discovers active WorkScope identities and paths through `query_database` before targeted Bash operations. ADR-066 records removal of the per-request snapshot and its dedicated runtime wiring.

The host-bound `query_database` tool provides operator-level forensic reads of Phoenix application tables, including hidden messages and sensitive records that may not be visible in normal UI. The database still exposes legacy project-named rows and fields; normative orientation now identifies ProductConversation and WorkScope, with repository context derived through WorkScope rather than a Project grouping. It executes one statement on a separate read-only connection. SQLite authorization denies mutation, connection-changing operations, internal and FTS shadow storage, filesystem and extension functions, while SQL/column/row/serialized-output/time bounds protect system stability. Results use typed cells and report truncation.

Natural-language message search, bounded transcript reads, and the shared cross-conversation message service are available to write-capable ordinary ProductConversations and the Coordinator. Durable reference resolution remains Coordinator-only. Restricted planning conversations and sub-agents receive none of these global tools. Coordinator also receives unsandboxed Bash with an explicit active WorkScope ID whose canonical cwd Phoenix resolves server-side. The Coordinator registry remains builtin-only and excludes ambient/default filesystem access, browser, MCP, task, repository, workspace, conversation creation, approval, and lifecycle mutation tools.

The separately specified ordinary-parent predecessor capability (REQ-RET-009,
ADR-051) does not grant global authority to restricted planning parents. It is
not implemented; its status and delivery task live in the conversation-retrieval
executive summary.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-GR-001:** Provide Transparent Current Activity Facts | 🟡 Partial | Current facts are requested through bounded SQL; no per-turn snapshot is injected. |
| **REQ-GR-002:** Expose Continuation Identity Without Collapsing Evidence | ✅ Complete | `@conv:<product_conversation_id>` reads the current transcript; `@transcript:<conversation_id>` pins an exact runtime member. |
| **REQ-GR-003:** Interpret Activity From Explicit Evidence | ✅ Complete | Prompt requires current relational state, timestamps, and recent evidence for status conclusions |
| **REQ-GR-004:** Provide Bounded Read-Only Relational Queries | ✅ Complete | Engine-authorized one-statement SQLite reads have work, row, and byte budgets |
| **REQ-GR-005:** Provide Stable References and App-Local Links | 🟡 Partial | Search and read output expose stable ProductConversation targets plus exact transcript/message citations; typed read/message targets remain separate from the legacy HTTP resolver compatibility surface. |
| **REQ-GR-006:** Provide One Durable Coordinator Identity | ✅ Complete | `/api/global/coordinator` resolves the singleton through the standard runtime and UI |
| **REQ-GR-007:** Bound Phoenix-Wide Agent Capabilities | ✅ Complete | Write-capable ordinary ProductConversations and Coordinator share bounded database/history reads and singular messaging; reference resolution and unsandboxed, WorkScope-targeted Bash remain Coordinator-only |
| **REQ-GR-008:** Answer With Source Citations | ✅ Complete | Search and transcript reads expose stable ProductConversation identity together with exact transcript/message evidence, and both Coordinator language prompts require that distinction. |
| **REQ-GR-009:** Resolve Durable Targets Without Guessing | ✅ Complete | Typed read/message tools share one canonical parser for `@conv:<product_conversation_id>` and `@transcript:<conversation_id>`, reject bare, WorkScope, chain, and malformed targets, and leave previously issued aliases to the separate compatibility resolver. `resolve_reference` reports the selected member's attached WorkScope lifecycle, environment, server paths, and explicit missing/unavailable status. |
| **REQ-GR-010:** Keep the Coordinator Surface Chat-First | ✅ Complete | `/global` mounts the shared conversation runtime, inline briefing action, bounded current-watch inventory, and process-local still-running Bash inventory; no open-work/current-attention/search view is added |
| **REQ-GR-011:** Obtain Current Activity on Demand | ✅ Complete | Stable Coordinator request instructions; existing bounded query tool supplies fresh facts and WorkScope discovery. Brief me requests a read-only check-in without polling. |
| **REQ-GR-011A:** Bound Database Integrity and Resource Use | ✅ Complete | Application data is readable; SQLite authority and resource budgets protect integrity and stability |
| **REQ-GR-012:** Commit and Report One Message Outcome | ✅ Complete | HTTP chat and agent cross-conversation actions share typed delivery, steering, rejection, and self-target rejection outcomes |
| **REQ-GR-013:** Preserve Coordination Context Through Compaction | ✅ Complete | Existing Coordinator identity selects coordination-specific system and summary instructions. The shared pipeline reserves the full accepted handoff before fitting recent history. Ordinary chats retain coding instructions and receive the same handoff protection. ADR-049 records the policy. |

## Verification Summary

The Coordinator dispatch regression checks that successive requests with changing transcript evidence retain exactly one stable system block and the same conversation cache key. Shared global-query and Bash-target tests retain coverage of explicit fact discovery and authoritative scope validation.

Compaction runtime, database, and property tests cover full accepted user-edited handoff retention, message-identity deduplication, absent provenance, reset/stale exclusion, provider budgets, assistant-leading recent history, and pre-dispatch recoverable failure. The opt-in `live_three_compaction_comparison` exercises three successive handoffs and diagnostic resumption probes under generic instructions, Coordinator instructions alone, and Coordinator instructions with protected input. It requires explicit model/output settings and is excluded from normal CI.

A bounded `gpt-5.5` comparison retained the paused workstream, owner succession, cancellation, deployment limit, and evidence distinctions through all three protected handoffs. Both unprotected arms lost earlier obligations when trimming removed their handoffs; Coordinator instructions alone improved next-action routing but could not recover omitted facts. This is one synthetic sample per arm, not a statistical quality guarantee or full runtime replay. Task 45014 records the fixture and interpretation.

Coverage verifies operator-level application-data reads, read-only SQLite authority, denied internal/filesystem/mutation operations, statement cardinality, SQL/column/row/serialized-output/work bounds, typed results, stable ProductConversation reads through the current transcript, exact transcript pinning, ambiguous target rejection, WorkScope target resolution and no-default behavior for Coordinator unsandboxed Bash, current-context app-local citation navigation with a Coordinator return origin, transcript paging, writing-versus-restricted tool boundaries, chat-first responsive layout, current watch add/remove refresh and readable identities, process-local Bash inspect/stop platform truth, shared message acceptance semantics, and self-target rejection without dispatch.

Reference-resolution coverage verifies point-in-time current-member and WorkScope selection for stable ProductConversation references during concurrent continuation, historical-member pinning for exact transcript references, active and retired lifecycles, allocated-worktree and unowned-cwd environments, worktree-preferred effective paths, cwd fallback, no-environment scopes, missing attachments, unavailable records, and explicit server-filesystem path semantics.

## Scope

The scope is on-demand relational orientation, one durable chat-first Coordinator conversation, bounded current-watch and still-running Bash activity surfaces, bounded global reads, explicitly WorkScope-targeted local operation through unsandboxed Bash, singular text-message delivery to existing non-Coordinator conversations, and a compact read-only briefing action.

The Coordinator runs on user turns and explicit watch notifications. Watches follow ordinary stable conversations across continuation and report future facts through ordinary admission; no ambient activity snapshot is restored. Notification acceptance does not establish execution or recipient understanding. The watch implementation is under qualification in PR #815.

## Out of Scope

- Database writes, attached databases, extensions, filesystem functions, or SQLite internal and FTS shadow storage.
- Phoenix-wide tools for restricted planning conversations or sub-agents.
- Images, files, skills, user-agent metadata, or lifecycle commands in cross-conversation messages.
- Batch-action transactions or atomic fan-out.
- Dedicated conversation-creation, task-lifecycle, approval, repository, filesystem, or workspace mutation tools for the Coordinator; user-authorized supported HTTP APIs remain available only through explicitly targeted Bash. Current APIs do not provide one aggregate-targeted lifecycle contract, uniform operation receipts, a general conversation retry, or caller-idempotent cancel.
- Ambient monitoring outside explicitly enrolled conversation watches.
- A separate transcript/composer runtime for the Coordinator.

## Input provenance and explicit watches qualification

REQ-GR-014 is implemented across trusted admission, steering, persisted history, search/resolve output, web and iOS attribution, and model-bound text. Historical unrecorded input remains unknown. Focused provenance tests and web checks pass; final exact-head CI/review remains the release gate.

REQ-GR-015 has explicit Global watch tools, normalized obligations, source-transaction production for direct and initial creation execution, continuation suppression, ordinary durable admission, and Close/unsubscribe ordering. Focused DB coverage includes future-only enrollment, accepted-input deduplication, continuation outcomes, cancelled Close, and creation execution. No subscription scheduler or pause-on-Stop mode is added. Cancellation packets report cancellation without fabricating a human actor where the persisted source lacks that attribution. End-to-end live idle/busy model execution and the full failure-path coverage remain qualification obligations, not implied by focused DB tests.
