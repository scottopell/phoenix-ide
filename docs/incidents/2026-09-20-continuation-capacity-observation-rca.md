# Continuation observation and provider-overload RCA — 2026-09-20

## Executive conclusion

The reported roughly three-hour `awaiting_continuation` condition for owners #775 and #789 was **not a production runtime event on `devmbp`**. The report combined observations whose deployment/source identity was not established and was contradicted by authenticated API, SQLite, active logs, rotated logs, and request metrics from the production deployment. This is an observation-provenance error, not evidence of a shared continuation, queue, restart, or capacity incident for those owners.

A separate defect was confirmed: deployed commit `d994618b41cd13090400833176e2bc63636bccc6` classified selected-model capacity responses as `ServerOverloaded` but did not automatically retry them. #764 and #777 each had one such attempt. No failed request metric tied overload to #775, #789, or #788 in the bounded evidence window. The corrective delivery therefore changes overload policy independently; it does not present overload as the cause of the corrected owner report.

## Evidence authority and handling

All collection ran read-only on `devmbp`. No `/trigger-continuation`, `/continue`, cancel, dismiss, retry, message, model change, runtime eviction, service restart, merge, or deployment was applied to #775 or #789.

| Evidence | Observed result |
|---|---|
| Authenticated production `/api/version` | `d994618b41cd13090400833176e2bc63636bccc6` |
| Delivery base after the required rebase | `87606f42404d8d169b85cea2f6de3e6732a3e58f` |
| Production SQLite | Online backup from read-only URI; `PRAGMA integrity_check = ok` |
| Database snapshot | 1,415,811,584 bytes; SHA-256 `21bac0ad90e43ff1983b3ca882522f7eec70f17dd062124422a117da264e7364` |
| Production log | Active log plus 31 gzip rotations searched for both UUIDs and lifecycle transitions |
| TraceQL | Read-only Tempo endpoint returned HTTP 200 with `{"traces": []}` for the bounded probe |
| Coordinator snapshots | Both named `/tmp` paths were unavailable on `devmbp`; retained as external references, not silently replaced |
| Secrets/content | SQL and committed artifacts contain metadata only. Message text, prompts, credentials, raw DB/logs, and trace payloads are excluded. |

Collection commands and bounded queries are summarized below. Physical raw evidence remains outside Git under `/tmp/phoenix-continuation-rca-20385ef3-20260920T140000Z`; the committed artifact contains only sanitized facts and hashes.

## Original observation and correction

The initial report was that #775 and #789 appeared to spend roughly three hours in `awaiting_continuation`. That report is preserved as the initiating observation; it is not promoted to an observed durable transition.

The following coordinator statement is preserved exactly:

> At 2026-09-20 13:46:42Z a read-only snapshot found #775 already idle (updated 06:15:18Z) and #789 newly llm_requesting because an ordinary reconciliation chat was admitted at 13:46:22Z; no `/trigger-continuation`, `/continue`, cancel, dismiss or retry was applied to #775/#789. A trigger check later observed both idle and deliberately made no mutation. #789's ordinary chat is review housekeeping, not incident recovery. Local coordinator evidence snapshot exists on the other host at `/tmp/phoenix-775-789-rca-presnapshot-20260920T134642Z`; Global has an independent read-only snapshot. Do not rely solely on mutable current API state.

The corrected coordinator snapshot is externally referenced as `/tmp/phoenix-rca-correction-20260920T135246Z`. Neither coordinator-host snapshot was available locally; authenticated `devmbp` evidence was independently frozen before analysis.

## Owner timelines

Labels mean:

- **Observed** — present in frozen SQLite, authenticated API, active/rotated production log, or request metrics.
- **Inferred** — best explanation consistent with observed evidence.
- **Missing** — source was unavailable or had no retained record; not treated as proof of the opposite.

### #775 — `e0fe6ae8-3382-4be8-a66b-96e33de1b484`

| Time (UTC) | Label | Event |
|---|---|---|
| Through 03:59:54.943818 | Observed | Rotated logs show ordinary `LlmRequesting ↔ ToolExecuting` work. No `AwaitingContinuation` transition appears in any searched active/rotated log for this UUID. |
| 06:15:18.569441 | Observed | Frozen `conversations` row and authenticated API agree on `idle`; `state_updated_at` and `updated_at` match. |
| Snapshot time | Observed | No `continued_in_conv_id`, no child successor row, zero queued steering rows, five durable turns, zero owning and zero unterminated turns. |
| Relevant day | Observed | No failed `llm_request_metrics` row for this UUID. |
| Before retained log bound / unretained traces | Missing | TraceQL returned no retained traces and external coordinator receipts were unavailable on `devmbp`. |

**Classification:** wrong/unverified observation source. No evidence supports a persisted wait, queue starvation, prepared handoff, provider backoff, owner shutdown, or recovery mutation for #775.

### #789 — `9846d10e-6215-424b-a188-89a999a252c3`

| Time (UTC) | Label | Event |
|---|---|---|
| 09:23:30.099073 | Observed | Request metric records successful attempt 1. |
| 09:23:30.107877 | Observed | Active log records `LlmRequesting → Idle`. |
| 13:46:22 | Observed | Coordinator admitted an ordinary review-housekeeping chat. This was not incident recovery. |
| 13:46:32.731616 | Observed | Ordinary request metric records success, attempt 1. |
| 13:46:53.269684 | Observed | Summary/model request `d8077f1b-00ea-4655-bb69-1ee94cd138e9` records success, attempt 1. |
| 13:46:53.276737 | Observed | The ordinary chat newly transitions `LlmRequesting → AwaitingContinuation`. This is the first `AwaitingContinuation` line in all searched logs for #789. |
| 13:48:56.135583 | Observed | Continuation compaction request records success, attempt 1, after 122,831 ms. |
| 13:48:56.138795 | Observed | Log records `AwaitingContinuation → ContextExhausted`; frozen SQLite records `context_exhausted` at `13:48:56.138785`. |
| Snapshot time | Observed | No successor, three durable turns, zero owning and zero unterminated turns, zero queued steering rows. |
| Historic pre-13:46 wait | Missing/contradicted | No matching active/rotated log transition and no failed request metric. External receipts and retained traces were unavailable. |

**Classification:** a real, new continuation summary ran for about 123 seconds after the coordinator's ordinary housekeeping chat and completed successfully. It is not a three-hour incident and did not recover an older incident. The older report came from an unverified observation source.

## Observation-provenance root cause

**Observed:** the production deployment identity was d994, while the local checkout initially cited for behavior was not that deployment and the managed branch was originally provisioned from an even older local base. Authenticated API/DB/log evidence did not match the reported three-hour state.

**Inferred:** the report conflated a coordinator/projection observation with production authority without first binding it to host, authenticated endpoint, deployed SHA, database path, conversation UUID, and observation time. The later ordinary #789 chat then produced a genuine `AwaitingContinuation`, making the earlier report appear plausible if chronology was not preserved.

**Missing:** the exact original command output and the two external snapshot directories were not available on `devmbp`, so the precise wrong machine/API/DB/projection cannot be named more narrowly.

Corrective operational rule: an incident-state assertion must carry `{host, authenticated API base, /api/version SHA, database identity, conversation UUID, UTC observation time}` and must be reconciled against durable state plus transition evidence before a recovery mutation.

## Capacity comparator

| Owner | Time UTC | Request | Transport | Deployed classification | Attempt | Evidence/conclusion |
|---|---:|---|---|---|---:|---|
| #777 | 01:17:44.079332 | `22f77123-eff8-4590-a0f0-b951e1e9f41c` | HTTP SSE | `server_overloaded` | 1 | Confirmed one-attempt terminal overload under d994. |
| #764 | 06:19:17.086508 | `114d6fca-1f89-434d-94fb-39ad0451379f` | WebSocket | `server_overloaded` | 1 | Confirmed one-attempt terminal overload under d994. |
| #788 | Relevant day | — | — | — | — | No failed metric in the bounded query. Absence is not universal proof. |
| #775 | Relevant day | — | — | — | — | No failed metric; no causal link to overload. |
| #789 | Relevant day | — | — | — | — | All bounded request metrics succeeded; no causal link to overload. |

The comparator establishes a policy gap, not a shared incident cause.

## Retry contracts

### Deployed d994

| Path | Classification and timing | Ownership/publication | Restart |
|---|---|---|---|
| Ordinary turn | Generic eligible errors: 3 total attempts, nominal 2s/4s. `ServerOverloaded`: `NoAutoRetry`, attempt 1 only. | Admission/direct turn remained authoritative through a provider attempt; terminal overload was visible and user-resumable. | Active durable-turn recovery governed ordinary work; no overload backoff existed to reconstruct. |
| Continuation summary/compaction | Persisted operation identity; generic eligible errors used 3 total attempts and 2s/4s. `ServerOverloaded` was terminal after one attempt. | Exhaustion published `RecoverableContinuationFailure`; summary commit was operation-fenced and idempotent. | Startup materialized persisted continuation operations; no overload window existed. |
| Continuation opening | Single durable opening intent routed through ordinary chat admission. | No independent summary retry loop; one successor/message identity. | Persisted intent was the recovery authority. |

### Delivered current-main correction

| Property | Contract |
|---|---|
| Scope | Same selected model; ordinary requests and continuation summaries share one typed `ServerOverloaded` policy. Continuation opening retains its one durable intent and gains no second loop. |
| Attempts | 5 total attempts. Generic retry remains 3 total attempts at nominal 2s/4s. |
| Delays | Nominal overload waits 4s/8s/16s/32s, with deterministic ±25% jitter derived from stable logical identity and target attempt. |
| Provider guidance | Standard `Retry-After` (seconds/date) and supported millisecond equivalents are typed separately from quota reset. A valid hint is a floor. A hint over 30s terminates automatic recovery visibly rather than being truncated. |
| Elapsed budget | Fixed 120s absolute window captured at first overload; never renewed by retry, reconnect, process restart, or policy routing. Dispatch and provider work are rejected/timed out at the same deadline. |
| Durability | Persisted typed waiting/in-flight state owns target, attempt, retry time, start, deadline, and continuation operation identity. Migration 102 expands the relational discriminator and marks the rollback boundary. |
| Visibility | SSE/UI shows `retry K/5 model overloaded` and countdown. Exhaustion clears live retry context and publishes ordinary `Error` or continuation `RecoverableContinuationFailure`. |
| Cancellation/Close | Timer generation and task are retired; stale timeout is ignored; fatal close releases admitted authority. |
| Restart | Waiting state rearms remaining time; due work dispatches once; expired state settles visibly without provider dispatch; in-flight work recovers under the original deadline. |
| Idempotence | Late/duplicate outcomes are fenced; continuation operation identity prevents duplicate summary/commit; continuation opening keeps one successor and handoff intent. |

Worst-case policy envelope is 120 seconds from the first overload classification, including all waits and subsequent provider attempts. The attempt cap may terminate sooner. This supersedes only the terminal-overload decision; quota/credits/authentication/prompt rejection/invalid request remain excluded, with no model substitution or billing action.

## Corrective implementation and receipts

The delivery was rebased once onto current `origin/main` (`87606f42404d8d169b85cea2f6de3e6732a3e58f`) after preserving the coherent stale-base unit. The d994 timeline above remains historical production truth; the fix targets current main.

Structural correction:

- preserves provider retry guidance in typed overload errors without conflating quota windows;
- represents overload retry as one persisted waiting/in-flight lifecycle;
- uses one shared reducer policy for ordinary and continuation summary work;
- expands the SQLite `state_kind` domain in migration 102 and preserves the state through startup reset/materialization;
- projects a narrow public/UI state rather than exposing the persisted continuation request;
- uses existing generation, operation, and direct-turn ownership fences for cancellation and duplicate suppression;
- updates normative LLM, retry-visibility, Bedrock, provider Allium, and ADR-059.

Deterministic regression receipts include:

- ordinary and continuation overload schedule/recover/exhaustion;
- stable jitter bounds and unchanged generic 2s/4s policy;
- provider `Retry-After` seconds/date/millisecond parsing, over-limit preservation, wrapped WebSocket and HTTP paths, and no quota conflation;
- persisted waiting/in-flight serialization, migration, reset preservation, and startup rearm/due/expiry classification;
- visible ordinary `Error` and continuation `RecoverableContinuationFailure` with stable wrapper attempt/operation identity;
- cancellation/fatal cleanup aborting the timer, bumping generation, ignoring stale timeout, and releasing admitted authority;
- sub-agent exhaustion/later error reaching `Failed` plus parent notification;
- duplicate overload expiry and late continuation outcomes producing no effects;
- global/UI busy, cancel, parser, focus, and retry-reason projection.

## Remaining evidence limits

- TraceQL returned no traces for the bounded query; it cannot corroborate earlier intervals.
- The two coordinator-host snapshot directories and Global's physical snapshot were not accessible on `devmbp`.
- Production logs preserve state transitions but do not attach operation/request IDs to every state-change line; DB request metrics provide the time correlation.
- No production mutation or deployment was performed, so correction receipts are deterministic tests on current main rather than live production experiments.

These limits do not support claiming universal absence. They are sufficient to reject the specific three-hour production assertion because all available production authorities agree on the corrected owner states and no contrary retained transition/request evidence exists.
