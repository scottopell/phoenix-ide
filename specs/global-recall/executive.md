# Phoenix Coordinator — Executive Summary

## Requirements Summary

Phoenix Coordinator is one durable, chat-first Phoenix-wide conversation for surveying unrelated work, inspecting relevant history, and sending useful text guidance to existing conversations. Phoenix supplies a transparent bounded relational snapshot on every Coordinator turn; the Coordinator retains bounded natural-language history search, conversation reading, read-only operational SQLite, and stable reference resolution.

The Coordinator has one mutation capability: a singular text-message action targeting an existing non-Coordinator conversation. That action reuses the normal chat acceptance authority, so each target independently reports delivered, queued as steering, or rejected. Acceptance never implies that the receiving agent understood, acknowledged, or completed the instruction.

The `/global` surface is the standard transcript and composer without a separate work view. A compact composer action requests a read-only current-activity briefing through the normal message path while preserving the user's draft.

## Technical Summary

Coordinator LLM requests keep the stable language-specific prompt as their cached prefix and append a bounded snapshot of raw continuation-leaf facts. The snapshot identifies both root and current conversations, exact runtime state, state and conversation timestamps, available task metadata, WorkScope identity, and authoritative active WorkScope paths. It applies no open, stalled, closed, or attention classification.

The Coordinator-only `query_database` tool provides operator-level forensic reads of Phoenix application tables, including hidden messages and sensitive records that may not be visible in normal UI. It executes one statement on a separate read-only connection. SQLite authorization denies mutation, connection-changing operations, internal and FTS shadow storage, filesystem and extension functions, while SQL/column/row/serialized-output/time bounds protect system stability. Results use typed cells and report truncation.

Natural-language message search, bounded transcript reads, durable reference resolution, and the shared cross-conversation message service remain specialized tools. When the host supports the Explore `nono` sandbox, Coordinator also receives the existing sandboxed Bash path with an explicit active WorkScope ID whose canonical cwd Phoenix resolves server-side. The registry remains builtin-only and excludes writable shell/filesystem access, browser, MCP, task, project, workspace, conversation creation, approval, and lifecycle mutation tools.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-GR-001:** Provide Transparent Current Activity Facts | ✅ Complete | Each turn receives raw current-leaf state, timestamps, WorkScope identity, and active environment paths without inferred work labels |
| **REQ-GR-002:** Expose Continuation Identity Without Collapsing Evidence | ✅ Complete | Snapshot and reference resolution expose both durable root and current conversation IDs |
| **REQ-GR-003:** Interpret Activity From Explicit Evidence | ✅ Complete | Prompt requires current relational state, timestamps, and recent evidence for status conclusions |
| **REQ-GR-004:** Provide Bounded Read-Only Relational Queries | ✅ Complete | Engine-authorized one-statement SQLite reads have work, row, and byte budgets |
| **REQ-GR-005:** Provide Stable References and App-Local Links | ✅ Complete | Work, chain, conversation, and message references remain durable and resolvable; Coordinator citations navigate within the current Phoenix context and expose the existing parent-style return breadcrumb |
| **REQ-GR-006:** Provide One Durable Coordinator Identity | ✅ Complete | `/api/global/coordinator` resolves the singleton through the standard runtime and UI |
| **REQ-GR-007:** Bound Phoenix-Wide Coordinator Capabilities | ✅ Complete | Database/history reads and WorkScope-targeted sandboxed Bash are Coordinator-only; one text-message mutation remains |
| **REQ-GR-008:** Answer With Source Citations | ✅ Complete | Transcript reads expose citation metadata and the prompt requires stable citations |
| **REQ-GR-009:** Resolve Durable Targets Without Guessing | ✅ Complete | Typed resolution supports work/conversation handles, links, IDs, and chain checks |
| **REQ-GR-010:** Keep the Coordinator Surface Chat-Only | ✅ Complete | `/global` mounts only the shared conversation runtime and inline briefing action |
| **REQ-GR-011:** Inject a Bounded Relational Snapshot | ✅ Complete | Requests append transparent turn-current facts and active WorkScope paths after the cached stable prompt |
| **REQ-GR-011A:** Bound Database Integrity and Resource Use | ✅ Complete | Application data is readable; SQLite authority and resource budgets protect integrity and stability |
| **REQ-GR-012:** Commit and Report One Message Outcome | ✅ Complete | HTTP chat and Coordinator action share typed delivery, steering, and rejection outcomes |

## Verification Summary

Coverage verifies operator-level application-data reads, read-only SQLite authority, denied internal/filesystem/mutation operations, statement cardinality, SQL/column/row/serialized-output/work bounds, typed results, raw continuation identities and active WorkScope paths, stable references, WorkScope target resolution and no-default behavior for Coordinator sandboxed Bash, current-context app-local citation navigation with a Coordinator return origin, transcript paging, Coordinator-only tools, chat-only responsive layout, and shared message acceptance semantics.

## Scope

The scope is transparent relational orientation, one durable chat-only Coordinator conversation, bounded global reads, WorkScope-targeted read-only local investigation through the existing Explore sandbox, singular text-message delivery to existing non-Coordinator conversations, and a compact read-only briefing action.

The Coordinator runs only on user turns. It does not monitor work in the background, create conversations, manage a global objective, retain attention history, or infer recipient understanding from message acceptance.

## Out of Scope

- Database writes, attached databases, extensions, filesystem functions, or SQLite internal and FTS shadow storage.
- Ambient Phoenix-wide tools for ordinary coding agents.
- Images, files, skills, user-agent metadata, or lifecycle commands in cross-conversation messages.
- Batch-action transactions or atomic fan-out.
- Conversation creation, task lifecycle, approval, repository, filesystem, project, or workspace mutation from the Coordinator.
- Background monitoring or proactive intervention without a user turn.
- A separate transcript/composer runtime for the Coordinator.
