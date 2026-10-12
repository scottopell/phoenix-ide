# WorkScope load — executive summary

## Measurement contract

This is sampled visibility for **live tracked jobs**, not exhaustive accounting. CPU snapshots may miss native children that start and exit between ticks. Attribution ends when the tracked handle terminalizes. `sampled_seconds` is represented observation duration, not completeness; zero live jobs means no tracked live jobs, not no surviving host processes. API `measurement_basis`, tool description and inline labels expose this boundary. Concrete detected lifecycle/membership/identity/freshness races still invalidate measurements. See [ADR-093](../adrs/093_sampled-live-job-resource-visibility.md).

## Scope and architecture

`WorkScopeLoadService` is runtime-owned and supplies the HTTP API and Global-only `work_scope_load` tool. The WorkScope section consumes the API, showing CPU in cores and memory inline. Attribution covers owned bash process groups only, including Coordinator jobs owned by a target WorkScope. Detached descendants, terminal/browser/shared host resources and disk are excluded.

Background collection reads only cumulative native CPU counters, on blocking workers, approximately every five seconds while jobs are live. A bounded in-memory interval ring yields time-weighted 1/5/15-minute values with sampled duration. Membership/identity changes and incomplete reads are gaps, not zero. CPU and memory require the observed group's launch leader PID/start-time match; a missing leader leaves surviving members unattributed and the affected current totals unavailable, without tracking detached descendants. History resets on server restart. Fresh memory uses the existing shared `ResourceMonitor` observation lease; the load service and inventory/inspection/deployment surfaces use the same monitor.

## Interfaces

- `GET /api/work-scope/:scope_key/load?conversation_id=...`: attached scope for ordinary callers; current Global may select another active scope. Unknown/inactive scopes are rejected.
- `GET /api/work-scopes/load?conversation_id=...`: current Global only; returns an array for active persisted WorkScopes with one shared demand generation.
- Global tool `work_scope_load`: optional `scope_key` (`work:<id>`); omitted means all active persisted scopes. Registration and execution-time role checks restrict it to the current Global.

`WorkScopeLoadSummary` carries typed `measurement_basis: "live_tracked_bash_groups"`, `scope_key`, `live_jobs`, latest cores, three `CpuLoadWindow` values (`average_cores`, `sampled_seconds`), nullable `cpu_sampled_at`, `sampled_at` for the demand snapshot, nullable `memory_bytes` and `process_count`. Timestamps are RFC3339 strings. Null is unavailable; a scope with no tracked live jobs has zero current tracked-job consumption and retains recent sampled windows, without asserting scope-wide native inactivity. A window without measurements has a null average and zero sampled duration.

The inline indicator is gated by confirmed live readiness, a nonarchived surface, and an exact matching attached inventory. Direct fixtures omit the capability and remain inert. Polling is bounded at five seconds, avoids overlap and ignores late responses. Sampled duration and separate CPU/memory age are in the title. No SSE event or automatic context injection is added.

## Verification

| Requirement | Implementation anchors | Coverage |
|---|---|---|
| REQ-WSLOAD-001 | `scope_cpu_members`, `cpu_deltas`, `summarize`, shared monitor launch identity | Synthetic reuse, absent launch leader with surviving member (CPU and memory), conflicting ownership, deduplication, missing-row and unrelated-gap tests |
| REQ-WSLOAD-002 | `WorkScopeLoadService::spawn_sampler`, `tick`, `window`, `prune_history` | CPU deltas, clipped/partial windows, restart-empty state, exited scope and idle pruning tests |
| REQ-WSLOAD-003 | `summary`, `summaries`, `ResourceMonitor::observe_live_groups` | Concurrent lease reuse and shared across-scope generation tests |
| REQ-WSLOAD-004 | `load_request_scope`, `contains_scope`, `WorkScopeLoadTool`, coordinator registry | HTTP attached/cross-scope admission, Global-only registration/execution and unknown-scope tests |
| REQ-WSLOAD-005 | `DesktopLayout`, `FileExplorerPanel`, `WorkScopeLoadIndicator` | Formatting, sampled-duration labels, no-tracked-live-jobs/null/stale values, response ordering, live/canonical/archived/pinned/inert-fixture tests |

Focused validation and equivalent base/candidate macOS qualification are recorded in task 68002. Rendered browser acceptance exercised the real ConversationPage → DesktopLayout → FileExplorerPanel → WorkScopeSection route: unmocked idle API, deterministic intercepted active/idle/stale/unavailable/partial responses in dark/light at 1440/1100 widths, keyboard Enter collapse/reopen, no overflow, and a real pinned historical route with zero load requests. Snapshot colors use secondary text for small-value contrast. No archived row existed in the local seed; archived admission remains covered by integration tests, not a separate rendered archived-row pass. No production deployment or acceptance is claimed.

Decision rationale: [ADR-092](../adrs/092_workscope-cpu-history-and-demand-memory.md). No Allium lifecycle model is added: this is a bounded read projection over existing resource lifecycles plus ephemeral measurement history, not a new work lifecycle.
