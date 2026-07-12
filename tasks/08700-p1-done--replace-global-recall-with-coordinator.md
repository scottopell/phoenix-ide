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

### Complete

- The normalized singleton `coordinator` relation identifies one ordinary persisted conversation without adding a discriminator to every conversation.
- `/api/global/coordinator` resolves or creates that durable identity; removed Recall session routes return 404.
- The Coordinator runs through the standard transcript, SSE, composer, persistence, context, and continuation machinery.
- Its runtime registry contains only `think` plus four host-bound, read-only tools: deterministic fleet inspection, global message search, paged conversation reads, and durable reference resolution. MCP, filesystem, shell, browser, task mutation, lifecycle, and sub-agent tools are structurally absent.
- Coordinator-specific system guidance establishes its fleet role, recommendation-only authority, citation behavior, deterministic-current-state preference, and on-demand operation.
- Shared global reads live in `api/global_read.rs`; both HTTP endpoints and normal runtime tools use the same service over `Database` and `MessageRetriever`.
- The bespoke Recall session storage handlers, locks, synchronous LLM loop, inline tool dispatcher, frontend session state, and obsolete tests are deleted. Migration 041 drops the unreleased Recall tables.
- `/global` embeds the ordinary conversation surface beside the compact, expandable, responsive fleet snapshot and excludes the Coordinator from open work.
- `./dev.py check` passes all 19 lanes. Browser QA verified stable singleton routing, the real composer, desktop/mobile fleet layout, detail expansion, API idempotence, and removed-route 404 behavior.

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
