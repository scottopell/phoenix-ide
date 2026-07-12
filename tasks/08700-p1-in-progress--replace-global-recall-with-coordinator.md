# Replace Global Recall with a durable Coordinator

## Goal

Replace the unreleased Global Recall product shape with a single durable **Coordinator** that helps the user run the fleet of Phoenix conversations.

The Coordinator is a structurally distinct global conversation built on the normal Phoenix conversation runtime and UI. It uses the standard transcript, streaming, composer, continuation/context-management behavior, and runtime persistence while receiving a deliberately bounded global tool set. It is not a project coding conversation and must not be representable as one accidentally.

This first release is intentionally on-demand and read-only. Approval-gated recommendations/steering and an event-driven attention inbox follow only after the durable workflow runtime work is complete.

## Product decisions

- The global surface’s primary job is fleet coordination, not historical investigation.
- There is exactly one durable Coordinator identity, not multiple user-created sessions.
- The Coordinator initially analyzes and recommends only when the user asks.
- It uses the normal conversation runtime rather than the bespoke synchronous Recall loop.
- It can read deterministic fleet state and search/read conversation history with citations.
- It cannot mutate projects, manage tasks, or steer other conversations in this task.
- Existing Global Recall session data may be dropped in the next migration. The feature is unreleased and no compatibility or data-preservation work is required.
- The existing deterministic open-work projection should be reused where it remains useful, but presented as an attention-first compact fleet list.

## UX

Make the Coordinator conversation/composer the center of `/global` (renamed in navigation and page copy). Beside or above it, show a dense fleet snapshot:

```text
FLEET · 8 active
  ⚠ Phoenix · CI fix · blocked · 4m
  ● Allium · spec migration · working · 12m
  ○ Website · deploy prep · waiting · 1h
```

Each row shows the minimum identity and status needed for orientation: project, work/conversation title, presentation state, recency, and key task/branch identity where useful. Expansion reveals existing audit detail such as inclusion signals, task metadata, branch, worktree, continuation root/current conversation, links, and references. Do not retain a session sidebar or “new recall session” flow.

The transcript and composer should reuse the ordinary conversation experience rather than maintain a parallel approximation. Coordinator-specific framing and fleet context may surround that shared experience.

## Implementation plan

1. **Revise the normative product contract first.** Replace the Global Recall requirements/executive framing with Coordinator terminology and behavior. Preserve requirements that still apply (deterministic fleet projection, durable references, bounded global reads, citations), remove multiple Recall-session requirements, and state the on-demand/read-only authority boundary. Update any navigation/product references. Do not add rollout/status language to timeless requirements.

2. **Model Coordinator identity structurally.** Add a typed conversation kind/scope or equivalent schema-backed discriminator that makes “the singleton global Coordinator” distinct from project/direct/work/branch conversations. Do not encode this as a magic title, nullable project convention, or an overloaded `ConvMode` if that would conflate tool/write mode with product identity. Enforce singleton lookup/creation transactionally and keep it out of ordinary project/open-work conversation lists.

3. **Use the standard runtime.** Route Coordinator turns through the existing conversation state machine, persistence, SSE streaming, transcript, composer, context handling, and continuation behavior. Supply Coordinator-specific system guidance and a Coordinator-specific tool registry selected from its typed identity.

4. **Extract/reuse bounded global read tools.** Reuse the existing deterministic open-work builder, global message search, paged conversation read, and reference resolution logic from `api/global_recall.rs` rather than duplicating it. Register only these host-bound read tools for the Coordinator. Keep ordinary coding conversations from receiving unrestricted global-history access. Preserve source metadata and app-local citations.

5. **Remove the bespoke Recall runtime and storage.** Delete Recall session CRUD/ask endpoints, synchronous answer loop, UI state, API contracts, locks, and tests. Add a migration that drops `global_recall_messages` and `global_recall_sessions`; no backfill is owed. Retain/refactor `/api/global/open-work` and reference resolution only as needed by the fleet UI/tools.

6. **Rebuild `/global` around the shared conversation experience.** Load/create the singleton Coordinator, render the standard transcript/composer behavior, and add the compact attention-first fleet snapshot with expandable detail. Rename sidebar/page labels from Global Recall to Coordinator. Avoid copying large conversation components if they can be cleanly shared or composed.

7. **Verify boundaries and regressions.** Cover singleton creation under concurrency, persistence/resume, runtime streaming, Coordinator-only global tools, absence from normal work lists, deterministic fleet grouping, compact/expanded UI behavior, citations/reference resolution, removal of Recall APIs, and schema migration. Run codegen where wire types change and run `./dev.py check`.

## Implementation snapshot

This task is **not complete**. Commits `97974b6e` and the subsequent singleton-relation correction establish the Coordinator foundation but stop at the most important backend integration boundary.

### Done

- Added a normalized singleton `coordinator` relation whose checked primary key permits exactly one row and whose unique foreign key identifies an ordinary conversation as the Coordinator. Ordinary conversations carry no Coordinator discriminator or redundant `standard` value.
- Added migration 041 creating that singleton relation and removing the unreleased `global_recall_sessions` / `global_recall_messages` tables.
- Added `GET`/`POST /api/global/coordinator` to atomically resolve or create the singleton Coordinator conversation.
- Routed the Coordinator through the standard conversation record, state machine, persistence, SSE transcript, and composer path.
- Added a Coordinator-specific runtime registry selected by membership in the singleton relation rather than title, project nullability, or `ConvMode`.
- Kept the Coordinator out of the ordinary conversation list and deterministic open-work projection.
- Replaced the Recall session UI/API client with a Coordinator page and renamed the navigation entry.
- Embedded the existing `ConversationPage` under `/global/:slug` so the normal transcript/composer remains on the same page as the fleet snapshot.
- Added the compact expandable fleet view and UI coverage for its default/detail states.
- Reframed `specs/global-recall/requirements.md` and `executive.md` around the Coordinator product.
- Targeted Coordinator/Sidebar UI tests pass; `cargo check`, TypeScript compilation, formatting, and `git diff --check` pass.

### Incomplete or incorrect

- The Coordinator registry currently contains only `think`. It is read-only, but it does **not** yet expose the required global capabilities:
  - deterministic fleet/open-work read
  - global message search
  - paged conversation read
  - reference resolution
- Those capabilities still exist as inline `ToolDefinition`/dispatch functions inside the retired bespoke Recall agent loop. They must become normal `Tool` implementations usable by the standard runtime. Until then, the Coordinator cannot perform its core job.
- `api/global_recall.rs` still contains the dead Recall session structs, handlers, locks, persistence functions, synchronous LLM loop, inline tool dispatcher, and associated tests. Routes and frontend callers are gone, but the backend implementation has not been deleted or cleanly separated into reusable fleet/history services.
- Coordinator-specific system guidance has not been added to the normal runtime prompt.
- The in-page reuse mounts route-aware `ConversationPage` under `/global/:slug`. This is functional reuse rather than a clean shared conversation-surface extraction and needs browser QA for layout/provider/route assumptions.
- Singleton identity and repeated get-or-create behavior are schema- and DB-tested, but concurrent callers and endpoint idempotence still lack dedicated tests.
- Migration 041, normal-list exclusion, open-work exclusion, standard runtime resume/streaming, removed Recall routes, and Coordinator tool isolation lack focused regression tests.
- A full `./dev.py check` has not passed for this implementation snapshot. Targeted compile/tests must be rerun after the singleton-relation correction, followed by the complete check.
- The task was deliberately returned to `in-progress`; do not mark it done until the global tools are integrated, dead Recall code is removed, focused tests exist, browser QA is complete, and `./dev.py check` passes.

### Recommended continuation sequence

1. Extract the four global read capabilities from `api/global_recall.rs` into a host-bound service with narrow dependencies (`Database` plus `MessageRetriever` where required).
2. Implement four standard runtime `Tool` types over that service and register exactly those tools plus `think` in `ToolRegistry::coordinator`; do not expose filesystem, browser, shell, task, sub-agent, MCP, or lifecycle mutation tools.
3. Thread the host-bound service into runtime/tool construction without adding global data to ordinary `ToolContext` or making unsupported capabilities representable for normal conversations.
4. Add Coordinator-specific system guidance covering fleet orientation, recommendation-only authority, and mandatory source citations.
5. Delete all Recall session storage/API/agent-loop code and its obsolete tests; retain only the deterministic fleet endpoint/reference endpoint needed by UI or refactor them over the shared service.
6. Add the missing DB/API/runtime/tool boundary tests, then run `./dev.py check`.
7. Start the app and browser-QA `/global` for transcript streaming, sending/cancelling, compact/expanded fleet layout, navigation, refresh, and responsive behavior.

## Explicitly deferred

- Background/event-driven fleet analysis or a durable attention inbox.
- Automatic intervention recommendations generated without a user turn.
- Approval UI, approval policy, auto-approval, or direct steering of conversations.
- Workflow subscriptions, durable queued coordinator work, retries, and partial-failure recovery.
- Multiple coordinators, mission sessions, or per-project coordinators.

These are follow-on Coordinator capabilities after the durable workflow runtime lands; this task should leave a clean typed extension point without prebuilding them.

## Acceptance criteria

- Opening Coordinator always resolves to the same durable global Coordinator identity.
- The Coordinator uses the normal Phoenix conversation transcript, streaming composer, persistence, and context/continuation machinery.
- It can deterministically inspect fleet state and search/read global conversation history with source citations.
- Its tools are read-only and unavailable to ordinary conversations by default.
- The default fleet view is compact and attention-first; detailed status/thread metadata is available through expansion rather than dominating the page.
- The Coordinator is excluded from ordinary project and open-work lists.
- Multiple Recall sessions and their custom UI/API/runtime no longer exist.
- A migration drops unreleased Recall session/message data without preservation machinery.
- No background inbox or steering capability is introduced in this task.
