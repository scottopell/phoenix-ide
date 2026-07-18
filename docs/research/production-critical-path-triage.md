# Production critical-path trace triage

## Decision summary

Production evidence identifies three user-blocking request paths worth immediate attention:

1. **PR status is the dominant measured latency source.** The UI already renders cached PR data and refreshes in the background, so this is primarily a performance and load problem—not an inbox/outbox migration. Stop recomputing GitHub/Git work on every poll; serve a durable cached snapshot and refresh it asynchronously.
2. **Conversation archive/delete and task completion cleanup belong on the durable-workflow adoption queue.** Their request handlers synchronously tear down processes, tmux, terminal/browser sessions, worktrees, branches, and attachments before acknowledging the user's lifecycle action.
3. **Chat acceptance needs a durable inbox boundary.** Most requests are fast, but an observed 3.42 s request spent 3.36 s on-CPU. The handler can initialize a runtime and expand filesystem references while holding the global chat-acceptance lock before returning an acceptance receipt.

The trace corpus does **not** support treating every long operation as a blocked HTTP journey. `conversation.turn` spans are deliberately long-running background work and are not endpoint latency.

## Evidence and method

VictoriaTraces was healthy at `127.0.0.1:10428` during this investigation. No collector, OTel, or trace-export warnings appeared in the last 3,000 production log lines.

Queries used the Tempo endpoint, service `phoenix-ide`, a seven-day maximum window ending **2026-07-18 15:32 UTC**, a maximum of 1,000 results per route, and full-trace fetches only for selected exemplars. Available matching data in the capped general HTTP sample ranged from **2026-07-18 02:21–09:12 UTC**. Counts marked `≥1,000` hit the result cap and are not traffic totals. Percentiles are descriptive of returned samples, not statistically complete service-level objectives.

### Ranked measured endpoints

| Rank | Critical journey | Endpoint | Returned samples | p50 / p95 / max | Representative trace | Blocking interpretation | Recommendation |
|---:|---|---|---:|---:|---|---|---|
| 1 | View active work / PR state | `GET /api/conversations/:id/pr-status` | ≥1,000 | 452 ms / 5.31 s / 6.64 s | `40ca42b27a5fe65c7e82af90d9536c64` (5.65 s) | Background refresh does not block navigation because `useConversationPrStatus` seeds cached data, but repeated synchronous `gh`/Git refreshes consume request workers and delay fresh controls. The exemplar was 5.64 s idle, consistent with external processes. | **Performance + async projection.** Return persisted PR snapshot immediately; refresh via one coalesced durable job per work scope; deliver freshness through the existing projection/SSE model. |
| 2 | Archive a conversation | `POST /api/conversations/:id/archive` | 10 | 90 ms / 3.71 s / 3.71 s | `800db1238f212c71c5b9e309a7b3b709` | UI awaits the request before removing/navigating. The exemplar was 3.60 s busy while the cleanup cascade ran. | **Durable workflow.** Persist lifecycle intent and acknowledge it before resource cleanup. |
| 3 | Send/steer a message | `POST /api/conversations/:id/chat` | 82 | 3 ms / 227 ms / 3.42 s | `d9c079f6649a4753225a1aa6ca031f94` | The composer awaits acceptance. The exemplar was 3.36 s busy; `SendChatApplicationService::send` may initialize a runtime and expand/validate references while holding the acceptance lock. | **Durable inbox + focused profiling.** Atomically persist accepted intent/idempotency identity, release the request, then expand and dispatch. |
| 4 | Render work-scope console | `GET /api/work-scope/:scope_key/inventory` | ≥1,000 | 3 ms / 261 ms / 442 ms | `3e2dc9488c1109323f6ad0f8ec5a9ff` | Read projection; slower samples divide roughly equally between busy and idle time. It should remain synchronous. | **Performance.** Add child timing only if this regresses; inspect resource sampling and query fan-out first. |
| 5 | Load conversation list | `GET /api/conversations` | ≥1,000 | 49 ms / 72 ms / 551 ms | `b22e589c5b3e0fef3bb3671d9137dbe6` | Initial navigation/read path. The outlier was mostly busy time but the common path is below 100 ms. | **Performance, lower priority.** Profile list enrichment only after the top three. |
| 6 | Load transcript page | `GET /api/conversations/:id/messages` | 115 | 1 ms / 17 ms / 43 ms | Route sample maximum | Healthy synchronous read. | Keep synchronous; no action. |

Additional bounded runtime-span search found 158 `conversation.turn` traces (p50 184 s, p95 1,264 s, max 3,611 s). These spans represent agent work during which the user can steer/cancel and therefore must not be compared with HTTP response latency. Searches by `name = "tool.execute"` and `name = "llm.request"` returned no standalone trace roots in this corpus; linked/child-span discoverability and deployed-version coverage need follow-up before drawing absence conclusions.

## Durable-workflow handoff queue

These candidates should adopt the existing durable-workflow stack rather than introduce endpoint-specific background schedulers.

| Priority | Boundary | Immediate acknowledgement | Durable state and progress | Completion/error delivery | Idempotency and side effects |
|---:|---|---|---|---|---|
| 1 | Archive/delete conversation and archive/delete chain | Record `requested` lifecycle intent and return `202` with workflow identity. UI hides the item or shows compact cleanup state immediately. | Profile covers busy-state validation, shared-scope ownership decision, resource cleanup, authoritative archive/delete commit, and terminal failure/manual-repair states. Chain operations fan out as profile-owned effects, not a request loop. | Persisted projection plus SSE; restart/reconnect reads the same state. Cleanup warnings remain visible and retryable. | Request idempotency key is conversation/chain + lifecycle generation. Effects include bash/tmux termination, terminal/browser shutdown, worktree/branch removal, attachment deletion, and final DB mutation. Preserve the existing work-scope ownership and checked-out-branch guards. |
| 2 | Abandon task / mark merged | Atomically accept the human decision and return the workflow identity; disable duplicate action immediately. | Profile covers PR/diff observation, state-machine acceptance, cleanup, final terminal state, and compensation/manual repair for ambiguous Git outcomes. | Persisted lifecycle state and SSE; errors must not become transient toasts only. | Decision + generation is idempotent. Side effects include diff capture, optional PR refresh, worktree/resource cleanup, and state transition. |
| 3 | Chat/steering acceptance | Atomically persist message intent, target, payload fingerprint, and acceptance receipt; return queued state without runtime initialization or expansion. | Profile/inbox distinguishes accepted, expanding, dispatched, rejected, and failed. Runtime reducer consumes the exact durable inbox item. | Existing conversation SSE/transcript projection; durable rejection/failure remains recoverable after restart. | Existing `message_id` fingerprint semantics are the natural key. Expansion, attachment validation/finalization, runtime creation, PR baseline capture, and state-machine dispatch move after acceptance with explicit compensation rules. |
| 4 | Conversation creation completion | Keep the existing durable creation-job acknowledgement and migrate remaining profile execution to the durable engine. | Creation profile already defines worktree and attachment effects. | Existing creation status and SSE. | Existing conversation/message IDs and creation job own retries. This is adoption completion, not a parallel design. |

`continue_conversation`, question answers, approvals/rejections, cancel, model changes, and small administrative mutations should keep a synchronous **acceptance** boundary. Their subsequent work can be asynchronous, but the response must authoritatively say whether the user's decision was committed.

## Performance queue (not durable-workflow migrations)

1. **PR projection:** split “read cached status” from “refresh external status.” Coalesce refreshes by work scope, apply a freshness budget, and avoid polling every open client independently. `useConversationPrStatus` already has cached-seed behavior and a 60-second poll, making this a compatible change.
2. **PR subprocess cost:** the current handler may perform branch lookup + work-change summary, direct active-PR lookup, and a retargeted lookup sequentially. Measure the newly added `pr_status.refresh{operation}` child spans before choosing between command consolidation, concurrency, or cache policy.
3. **Chat on-CPU outlier:** measure reference expansion, attachment validation, runtime `get_or_create`, event dispatch, and lock wait separately. Do not optimize the state-machine turn; optimize time-to-durable-acceptance.
4. **Inventory tail:** inspect process/resource sampling and DB fan-out if p95 remains above 250 ms after higher-priority changes.
5. **Conversation list outliers:** add bounded enrichment timings if >250 ms becomes frequent; current p95 does not justify architecture work.

## Trace coverage findings

### Implemented here

A new exported child span, `pr_status.refresh`, wraps each blocking PR refresh operation with a bounded `operation` value:

- `branch_and_work_change`
- `active_pr`
- `retargeted_pr`

It inherits the HTTP parent, exports no events or repository/PR identity, and is covered by the OTel allowlist regression test. This fills the highest-value gap encountered: existing traces showed a 5–6 second endpoint wait but no child work explaining it.

### Remaining gaps

- **Chat acceptance:** the HTTP span has no children and cannot distinguish lock wait, runtime startup, reference expansion, validation, DB acceptance, or dispatch. Add bounded child spans before/with migration.
- **Lifecycle cleanup:** archive/delete/abandon/merge traces have no child phases. A durable profile should emit workflow/effect spans linked to the acceptance request rather than extending the HTTP parent across background work.
- **Trace search:** VictoriaTraces search summaries did not include fields requested through `select(...)`, although full traces contained `http.route`. Route-by-route queries were required. Keep reproducible query scripts bounded and avoid assuming summary projections work.
- **Standalone tool/LLM discovery:** no roots matched name-only searches in the sampled window even though turn traces exist. Verify deployed exporter/version and span parent/link behavior before changing instrumentation.
- **Exporter backpressure:** no warning was observed, but the application does not expose dropped/export-failure counts in its normal operational surface. Existing follow-up task `44005` remains relevant.

Do not add conversation IDs, paths, branch names, PR identities, prompts, tool arguments, or payloads merely to improve search. Route templates, bounded operation discriminants, status, and causal links are sufficient.

## Reproduction queries

```bash
BASE=http://127.0.0.1:10428/select/tempo
NOW=$(date +%s)
START=$((NOW-7*86400))

# Bounded route sample
curl -fsSG "$BASE/api/search" \
  --data-urlencode 'q={ resource.service.name = "phoenix-ide" && name = "http" && span.http.route = "/api/conversations/:id/pr-status" }' \
  --data-urlencode "start=$START" --data-urlencode "end=$NOW" \
  --data-urlencode 'limit=1000'

# Identify slow HTTP exemplars before fetching full traces
curl -fsSG "$BASE/api/search" \
  --data-urlencode 'q={ resource.service.name = "phoenix-ide" && name = "http" && duration > 250ms }' \
  --data-urlencode "start=$START" --data-urlencode "end=$NOW" \
  --data-urlencode 'limit=100'

# Fetch only a selected trace
curl -fsS "$BASE/api/traces/40ca42b27a5fe65c7e82af90d9536c64"
```

Repeat measurements after instrumentation deploy using a narrow post-deploy window. Compare raw returned samples and report caps; do not present capped search results as total traffic or complete percentiles.
