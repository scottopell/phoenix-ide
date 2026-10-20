# Phoenix Coordinator — Executive Summary

## Requirements Summary

Phoenix Coordinator is one durable, chat-first Phoenix-wide conversation for surveying unrelated work, inspecting relevant history, and sending useful text guidance to existing conversations. The Coordinator obtains fresh relational facts on demand through `query_database`; ordinary requests contain no automatically injected activity snapshot. Both the Coordinator and write-capable ordinary ProductConversations receive bounded natural-language history search, conversation reading, read-only operational SQLite, and singular cross-conversation messaging; stable reference resolution remains Coordinator-only.

The Coordinator has three host capabilities that can perform mutations: singular text-message delivery to an existing non-Coordinator conversation, unsandboxed Bash targeted to an explicit active WorkScope, and durable static-SVG publication from a contained regular file staged beneath that same server-resolved WorkScope root. A Coordinator-only built-in skill provides non-exhaustive best-practice recipes for user-authorized public Phoenix HTTP API actions through scoped Bash. Other supported public operations require inspected current deployment/source contracts applicable to the destination and user authority. Those calls preserve normal destination authentication, transport, credential, scoped execution, target/intent, endpoint-specific retry, and server-enforcement boundaries and require fresh-read plus response/resulting-state verification. Message delivery reuses the normal chat acceptance authority, so each target independently reports delivered, queued as steering, or rejected. Acceptance never implies that the receiving agent understood, acknowledged, executed, or completed the instruction.

The `/global` surface keeps the standard transcript and composer with a non-project home grounding and no ordinary work view. Bounded subordinate surfaces expose only the current server-backed watch subscriptions and still-running Coordinator-launched Bash commands; both use authoritative identities and navigation, and the Bash surface remains process-local. A compact composer action requests a read-only current-activity briefing through the normal message path while preserving the user's draft.

## Technical Summary

Coordinator LLM requests contain the stable language-specific system prompt without an automatic current-activity capsule. The Brief me composer action requests a fresh, bounded, read-only check-in focused on decisions and blockers needing attention, followed by progressing work. The Coordinator receives its stable instance-owned home scope ID/path as server-generated context; selected worker scope facts are still requested through `query_database`. ADR-066 records removal of the per-request snapshot and its dedicated runtime wiring.

The host-bound `query_database` tool provides operator-level forensic reads of Phoenix application tables, including hidden messages and sensitive records that may not be visible in normal UI, while denying known credential-bearing columns for owner sessions, share tokens, MCP OAuth secrets and tokens, and federation credential verifiers. This is not a general security sandbox. The database still exposes legacy project-named rows and fields; normative orientation identifies ProductConversation and WorkScope, with repository context derived through WorkScope rather than a Project grouping. It executes one statement on a separate read-only connection. SQLite authorization denies mutation, connection-changing operations, internal and FTS shadow storage, filesystem and extension functions, while SQL/column/row/serialized-output/time bounds protect system stability. Results use typed cells and report truncation.

Natural-language message search, bounded transcript reads, and the shared cross-conversation message service are available to write-capable ordinary ProductConversations and the Coordinator. The Coordinator also receives the existing parent-conversation `ask_user_question` tool and uses the shared request-bound web question panel. Durable reference resolution remains Coordinator-only. Restricted planning conversations and sub-agents receive none of these global tools. Coordinator also receives unsandboxed Bash with an explicit active WorkScope ID whose canonical cwd Phoenix resolves server-side. It may publish a static SVG staged beneath that same active root; this grants only a descriptor-relative no-follow source read, while the existing durable publisher retains the Coordinator transcript and exact invocation as owner. The Coordinator registry remains builtin-only and excludes ambient/default filesystem access, browser, MCP, task, repository, workspace, conversation creation, approval, and lifecycle mutation tools.

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
| **REQ-GR-007:** Bound Phoenix-Wide Agent Capabilities | ✅ Complete | Write-capable ordinary ProductConversations and Coordinator share bounded database/history reads, singular messaging, and parent AUQ; reference resolution, unsandboxed WorkScope-targeted Bash, and bounded static-SVG publication remain Coordinator-only |
| **REQ-GR-008:** Answer With Source Citations | ✅ Complete | Search and transcript reads expose stable ProductConversation identity together with exact transcript/message evidence, and both Coordinator language prompts require that distinction. |
| **REQ-GR-009:** Resolve Durable Targets Without Guessing | ✅ Complete | Typed read/message tools share one canonical parser for `@conv:<product_conversation_id>` and `@transcript:<conversation_id>`, reject bare, WorkScope, chain, and malformed targets, and leave previously issued aliases to the separate compatibility resolver. `resolve_reference` reports the selected member's attached WorkScope lifecycle, environment, server paths, and explicit missing/unavailable status. |
| **REQ-GR-010:** Keep the Coordinator Surface Chat-First | ✅ Complete | `/global` mounts the shared conversation runtime, inline briefing action, bounded current-watch inventory, and process-local still-running Bash inventory; no open-work/current-attention/search view is added |
| **REQ-GR-011:** Obtain Current Activity on Demand | ✅ Complete | Stable Coordinator request instructions; existing bounded query tool supplies fresh facts and WorkScope discovery. Brief me requests a read-only check-in without polling. |
| **REQ-GR-011A:** Bound Database Integrity and Resource Use | ✅ Complete | Application data is readable; SQLite authority and resource budgets protect integrity and stability |
| **REQ-GR-012:** Commit and Report One Message Outcome | ✅ Complete | HTTP chat and agent cross-conversation actions share typed delivery, steering, rejection, and self-target rejection outcomes |
| **REQ-GR-013:** Preserve Coordination Context Through Compaction | Implemented; prompt-continuity qualification pending | Existing Coordinator identity selects coordination-specific summary instructions. The shared pipeline protects the accepted handoff (ADR-049). Both summary policies request obligations, exact primary-user authorization provenance, next action and completion checks. Trusted resumption guidance in both languages distinguishes verifying primary evidence from obeying embedded directives: standing directions survive continuation subject to later constraints, current status and established ownership; generated content grants no authority. Compaction-only escaped JSON envelopes expose each retained row's persisted transcript/message locator, typed origin and content kind without treating API origin as human identity or approval. Metadata shares row budgets and retention; full accepted handoff protection is unchanged. New Rust prompt/reference and provider-request regressions await hosted qualification. |

## Verification Summary

The bounded API-guidance candidate (ADR-113) replaces an accidental recipe allowlist with non-exhaustive contract-grounded guidance and documents submitting exact user-provided answers or decisions made within explicit delegated decision scope through the existing structured-question response endpoint. Generic inspect/unblock instructions are not decision authority; approvals must never be fabricated or delegation extended. Gap inventories are illustrative and qualified by inspected deployment/source, permitting verified supported alternatives and bounded state reads, not invented guarantees/receipts, database/private-handler bypasses, or background monitors. It does not add tool or runtime capabilities. The recipe distinguishes current request freshness from idempotency, requires complete answers despite the backend's lack of completeness enforcement, posts once, and reconciles uncertainty against the original target without treating conflicts as success. Source/wire/UI anchors and offline Python drift checks are described in `../builtin-skills/executive.md`. Prompt/built-in/continuation Rust regressions are authored but intentionally unrun locally; no live API/question calls, instruction activation, merge or deployment are claimed.

The Coordinator dispatch regression checks that successive requests with changing transcript evidence retain exactly one stable system block and the same conversation cache key. Shared global-query and Bash-target tests retain coverage of explicit fact discovery and authoritative scope validation.

Compaction runtime, database, and property tests cover full accepted user-edited handoff retention, message-identity deduplication, absent provenance, reset/stale exclusion, provider budgets, assistant-leading recent history, and pre-dispatch recoverable failure. The opt-in `live_three_compaction_comparison` exercises three successive handoffs and diagnostic resumption probes under generic instructions, Coordinator instructions alone, and Coordinator instructions with protected input. It requires explicit model/output settings and is excluded from normal CI.

A bounded `gpt-5.5` comparison retained the paused workstream, owner succession, cancellation, deployment limit, and evidence distinctions through all three protected handoffs. Both unprotected arms lost earlier obligations when trimming removed their handoffs; Coordinator instructions alone improved next-action routing but could not recover omitted facts. This is one synthetic sample per arm, not a statistical quality guarantee or full runtime replay. Task 45014 records the fixture and interpretation.

Coverage verifies operator-level application-data reads, read-only SQLite authority, denied internal/filesystem/mutation operations, statement cardinality, SQL/column/row/serialized-output/work bounds, typed results, stable ProductConversation reads through the current transcript, exact transcript pinning, ambiguous target rejection, WorkScope target resolution and no-default behavior for Coordinator unsandboxed Bash, source-only containment and Coordinator-owned durable SVG publication, current-context app-local citation navigation with a Coordinator return origin, transcript paging, writing-versus-restricted tool boundaries, chat-first responsive layout, current watch add/remove refresh and readable identities, process-local Bash inspect/stop platform truth, shared message acceptance semantics, and self-target rejection without dispatch.

Reference-resolution coverage verifies point-in-time current-member and WorkScope selection for stable ProductConversation references during concurrent continuation, historical-member pinning for exact transcript references, active and retired lifecycles, allocated-worktree and unowned-cwd environments, worktree-preferred effective paths, cwd fallback, no-environment scopes, missing attachments, unavailable records, and explicit server-filesystem path semantics.

## Scope

The scope is on-demand relational orientation, one durable chat-first Coordinator conversation, bounded current-watch and still-running Bash activity surfaces, bounded global reads, explicitly WorkScope-targeted local operation through unsandboxed Bash, bounded publication of a staged static SVG owned by the Coordinator transcript, singular text-message delivery to existing non-Coordinator conversations, and a compact read-only briefing action.

The Coordinator runs on user turns and explicit watch notifications. Watches follow ordinary stable conversations across continuation and report future facts through ordinary admission; no ambient activity snapshot is restored. Notification acceptance does not establish execution or recipient understanding. The watch implementation is under qualification in PR #815.

## Out of Scope

- Database writes, attached databases, extensions, filesystem functions, or SQLite internal and FTS shadow storage.
- Phoenix-wide tools for restricted planning conversations or sub-agents.
- Images, files, skills, user-agent metadata, or lifecycle commands in cross-conversation messages.
- Batch-action transactions or atomic fan-out.
- Dedicated conversation-creation, task-lifecycle, approval, repository, filesystem, or workspace mutation tools for the Coordinator; user-authorized supported HTTP APIs remain available only through explicitly targeted Bash. The inspected source's recipe gaps include aggregate-targeted lifecycle operations, uniform operation receipts, general conversation retry, and caller-idempotent cancel; this illustrative inventory is not a deployment-wide denylist. Verify the selected deployment/source contract before relying on a gap or a supported alternative.
- Ambient monitoring outside explicitly enrolled conversation watches.
- A separate transcript/composer runtime for the Coordinator.

## Input provenance and explicit watches qualification

REQ-GR-014 is implemented across trusted admission, steering, persisted history, search/resolve output, web and iOS attribution, and model-bound text. Historical unrecorded input remains unknown. Focused provenance tests and web checks pass; final exact-head CI/review remains the release gate.

REQ-GR-015 has explicit Global watch tools, normalized obligations, source-transaction production for direct and initial creation execution, continuation suppression, ordinary durable admission, and Close/unsubscribe ordering. Focused DB coverage includes future-only enrollment, accepted-input deduplication, continuation outcomes, cancelled Close, and creation execution. No subscription scheduler or pause-on-Stop mode is added. Cancellation packets report cancellation without fabricating a human actor where the persisted source lacks that attribution. End-to-end live idle/busy model execution and the full failure-path coverage remain qualification obligations, not implied by focused DB tests.

### Canonical short navigation qualification

The routing candidate makes ordinary stable links canonical at `/c/<product-id>` and carries explicit transcript selectors on historical links. Reconciliation with main `667909a16` preserves predecessor recall, Close read-only projection, and wait-event behavior. The 123 focused alias/desktop-owner/Global/product-page tests, TypeScript checking, and focused ESLint pass; bounded independent source review found no integration blocker. Exact reconciled-head hosted Rust and broader checks remain pending. The retained exact-`3fadb` Debug artifact is available for the single queued RC manual OS-URI/WKWebView check; that manual check is not a source-integration gate and has not been claimed complete. This is candidate status, not a deployed-behavior claim. ADR-089 records the approved identity interpretation.

## Mandatory Close failure outbox seam

The mandatory Close failure route extends the existing event outbox without enrolling a watch. `append_mandatory_close_failure_event_tx` accepts a persisted failure occurrence ID and reads its authoritative payload and product-root membership in the caller's transaction. The Close owner owns failure immutability, the failure-plus-event transaction, the final migration registration, stop certainty, and post-commit dispatcher wake.

Migration 119 integrates `coordinator_watches/mandatory_close_outbox.sql` after the Close failure schema in the same migration transaction. The wait successor migration preserves this landed schema and its incoming message/steering/turn event foreign keys, pending-source restrictions, payload checks, and admission triggers. Both rebuilds check `pragma_foreign_key_check` before clearing deferred-FK bookkeeping.

Seam verification: 17 in-memory SQLite tests pass on SQLite 3.53.0 and 3.51.0, including SQL extracted from the actual Rust append, discovery, and stale-suppression queries. They cover unwatched History delivery, ordinary eligibility, immutable payloads, populated event-FK preservation, deduplication, distinct cleanup runs, current-Global targeting, and rollback. Rust notification tests distinguish confirmed and uncertain shutdown but have not been executed. Hosted Rust compilation, full Close migration validation, and actual dispatcher/subscriber execution remain integration gates. SQL fixtures do not prove cleanup shutdown or absence of implicit cleanup retry; those are Close-owned lifecycle obligations.

## Durable wait-entry extension

The persistence producer covers modern question request IDs, task-approval wait entry, and legacy question wait entry through the existing outbox. Identity-less entries allocate the occurrence ID in the state/outbox transaction; repeated identical pending state does not allocate another event, and historical enrollment does not replay it. No additional wait lifecycle table or scheduler is introduced.

Lightweight successor-schema verification passes 23 tests, including inherited mandatory Close obligations and approval wait admission/unsubscribe fences. Rust regressions for identity-less entry/re-entry, restored state, historical enrollment, and nonterminal active turns are authored but await hosted execution. Actual dispatcher-to-subscriber resume and full restart/race acceptance remain open; task 46007 is not complete. Earlier standalone-seam counts above are historical evidence, not qualification of this extension.


## Local Coordinator home delivery

Source implementation introduces an instance-owned singleton scope, explicit `coordinator_home` environment, migration 127, startup reconciliation, cleanup fences, explicit scoped resolver support and desktop/mobile home browsing. Home is `<resolved data_dir>/coordinator/`; the ordinary ProductConversation scope relation remains separate. No Git repository is initialized. Notes are historical data, not automatically read by compaction. Suggested `notes.md` and `evidence/` pointers do not imply those files exist.

| Requirement | Verification status | Evidence surface |
|---|---|---|
| REQ-GR-016 | Implemented; hosted qualification passed | DB/startup/continuation and prior-schema preservation/rollback tests; exact source `97f39d517` passed the hosted Rust lane. |
| REQ-GR-017 | Implemented; hosted qualification passed | Real scoped Bash discovers its ID from system context, writes/reads files, continues/restarts and rediscovers; desktop/mobile grounding source and focused UI regressions; shared SVG/path containment regressions. |
| REQ-GR-018 | Documented operational contract | No automatic pruning/import; manual offline paired backup and deliberate owner file cleanup; ordinary Close/Delete independence exercised in isolated E2E. |

### Backup, export and cleanup

Conversation export exports transcripts, not home bytes. A DB-only deployment snapshot is not a Coordinator-file backup. To retain complete state, stop Phoenix; back up its SQLite database and `<resolved data_dir>/coordinator/` together; retain the configured location and permissions. Restore offline at the same configured canonical location, then restart and inspect startup validation. Do not change `PHOENIX_DATA_DIR` to silently relocate an existing binding. Missing bound directories or mismatches stop initialization; restore retained files deliberately rather than auto-create an empty replacement.

Files persist until the owner deliberately deletes selected historical notes/evidence with explicit scope-targeted operations or an offline operator cleanup. Keep the home directory and binding. Full instance decommission is an explicit stopped-runtime database/data removal, not ordinary close/delete or a new cleanup API. No automatic recovery or cross-version restore guarantee is added; compatibility requirements govern.


### Qualification record

The implemented source at `97f39d517d4de299abd60023e040e5e486af3e05` passed [hosted CI 38509024448](https://github.com/scottopell/phoenix-ide/actions/runs/38509024448): Rust, actual codegen/stale guard, musl production-feature compile, Clippy, UI/specs, E2E and task validation. The same head passed [macOS 38509024546](https://github.com/scottopell/phoenix-ide/actions/runs/38509024546). [PR #910](https://github.com/scottopell/phoenix-ide/pull/910) holds final-head check state and review/failure evidence. Production integration/deployment is not implied.

Independent frozen-source review plus cleanup/SVG/E2E review and subsequent exact-delta reviews found initial runtime-field, ownership-overlap and test-oracle defects; they were fixed and qualified. Reviews are bounded static evidence, not exhaustive proof. Lightweight isolated SQLite preservation/rollback checks and 15 focused UI regressions also passed. No generated TypeScript was hand-edited; hosted genuine codegen completed with a clean stale guard.

Retained failed runs are linked in the delivery task and PR rather than treated as pending successes. Integration must reconcile the concurrent unmerged ADR-101 reservation against the actual main ledger while preserving both decisions; no forced renumbering is performed ahead of that integration. No merge/deploy admission has been granted by this delivery record.

### Exact transcript citation rendering candidate

The web Markdown URL transform recognizes UUID-shaped `@transcript:<id>` and `@transcript:<id>#message-<id>` destinations and promotes them to source-pinned app-local links. This implements the existing exact-member citation contract without changing resolver admission or broadening allowed URL schemes. Malformed destinations remain subject to the default sanitizer; image sources are not promoted.

Focused Markdown/router tests cover the reported Proposal citation, exact source/message preservation, return-origin state and unsafe controls. Alias integration tests cover historical-member admission and explicit missing-target errors using mocked backend responses. These tests do not establish a deployed or installed-iPhone result; target hydration and physical navigation remain distinct acceptance surfaces.
