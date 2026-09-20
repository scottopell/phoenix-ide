# RCA of continuation observations and bounded overload retry correction

Commission exactly one incident workstream, run only on `devmbp` in its managed worktree, to explain the reported roughly three-hour `awaiting_continuation` observations for existing owners **#775** (`e0fe6ae8-3382-4be8-a66b-96e33de1b484`) and **#789** (`9846d10e-6215-424b-a188-89a999a252c3`), independently assess the confirmed provider-overload retry-policy gap, deliver the smallest justified correction, and qualify it through hosted review. Preserve both original implementation owners, WorkScopes, worktrees, and branches; do not take over their source scope, mutate their branches, or create successors for them.

The three-hour report is an **observation to explain**, not an established runtime event. Primary `devmbp` evidence presently contradicts it. The RCA must be willing to conclude that the coordinator observed the wrong source, machine, database, conversation projection, or time interval if that is what the preserved evidence establishes. Do not force a continuation or capacity cause for #775/#789.

## Corrected observed journey

### Primary `devmbp` observations

- Authenticated production `/api/version` reports deployed commit `d994618b41cd13090400833176e2bc63636bccc6` (`d994618b41cd`), not local/main `87606f42404d8d169b85cea2f6de3e6732a3e58f` (`87606f42`). The exact deployed commit is available in local Git. Reconstruct behavior and topology from that tree before citing current-main symbols or behavior; then separately map any forward-port needed on the delivery base.
- Read-only `prod.db` and authenticated API agree that #775 was idle since `2026-09-20 06:15:18.569441Z` and had no successor.
- Active `prod.log` records #789 transitioning `LlmRequesting → Idle` at `2026-09-20 09:23:30.107877Z`.
- The coordinator's ordinary review-housekeeping chat for #789 began at `2026-09-20 13:46:22Z` and created a **new** `AwaitingContinuation` at `2026-09-20 13:46:53.276737Z`, operation `d8077f1b-00ea-4655-bb69-1ee94cd138e9`, attempt 1. It completed to `context_exhausted` at `2026-09-20 13:48:56.138785Z`.
- No historical `AwaitingContinuation` line for #775 or #789 has yet been found in active `prod.log`. This is not proof of absence: search rotated logs, database request/attempt records, retained traces, and prior coordinator tool receipts first.
- Preserve the corrected coordinator snapshot at `/tmp/phoenix-rca-correction-20260920T135246Z` and Global's independent evidence as external references. If unavailable from `devmbp`, record that as missing evidence; do not execute work on another host.

### Earlier report and coordinator mutations

Do not erase or silently rewrite the earlier mistaken report. Preserve it in the RCA as the initial observation, followed by the correction and source-identity analysis. Preserve this coordinator mutation statement exactly:

> At 2026-09-20 13:46:42Z a read-only snapshot found #775 already idle (updated 06:15:18Z) and #789 newly llm_requesting because an ordinary reconciliation chat was admitted at 13:46:22Z; no `/trigger-continuation`, `/continue`, cancel, dismiss or retry was applied to #775/#789. A trigger check later observed both idle and deliberately made no mutation. #789's ordinary chat is review housekeeping, not incident recovery. Local coordinator evidence snapshot exists on the other host at `/tmp/phoenix-775-789-rca-presnapshot-20260920T134642Z`; Global has an independent read-only snapshot. Do not rely solely on mutable current API state.

Do not claim that #789's ordinary chat recovered the reported incident. Do not apply any incident recovery mutation to #775/#789 before the evidence package is frozen; current evidence does not indicate that either owner needs recovery.

## Verified deployed-source and database findings

These are anchors from exact deployed commit `d994618b41cd`, not current-main extrapolations:

- `crates/phoenix-core/src/domain/llm_error_kind.rs` classifies `ServerOverloaded` as `NoAutoRetry` but user-resumable.
- `crates/phoenix-llm/src/openai.rs` maps OpenAI/Codex `server_is_overloaded` and `slow_down` to `ServerOverloaded`; tests assert that this classification is terminal for automatic retry.
- `crates/phoenix-state-machine/src/transition.rs` defines `MAX_RETRY_ATTEMPTS = 3`. Its eligible-error retry path starts at attempt 1 and schedules attempts 2 and 3 after 2s and 4s. Those attempts do **not** apply to `ServerOverloaded` in deployed d994.
- Deployed d994 already has durable continuation operation identity, `AwaitingContinuation`, startup `resume_pending_continuations`, recoverable failure, stale-result fencing, and idempotent continuation commit. Their adequacy for this incident still requires evidence; their mere presence does not prove the reported wait occurred.
- Read-only `llm_request_metrics` records #764 request `114d6fca-1f89-434d-94fb-39ad0451379f` at `2026-09-20 06:19:17.086508Z` and #777 request `22f77123-eff8-4590-a0f0-b951e1e9f41c` at `2026-09-20 01:17:44.079332Z` as `server_overloaded`, attempt 1 only.
- No failed request metrics for #775, #789, or #788 have been found for the relevant day. Preserve that as a bounded negative query result, not universal proof that no request failed.

The confirmed independent policy question is therefore whether explicit transient model overload should receive a separate bounded automatic retry policy. It is **not** whether the generic three-attempt budget should simply be increased.

## Authorities and interaction map

At deployed d994, distinguish three paths:

1. **Ordinary turn:** admitted message/queue ownership → ordinary provider request → typed result → eligible retry or durable visible terminal state.
2. **Continuation summary/compaction:** persisted stable operation and inputs → tool-less provider request → eligible retry or visible recoverable continuation failure → atomic idempotent summary commit.
3. **Continuation opening:** context-exhausted owner → single successor plus durable opening-handoff intent → ordinary chat admission/queue → intent consumed by durable message acceptance. It is not the summary request and must not inherit a second retry authority.

For incident observation, separately map: coordinator/tool output → host and endpoint → authenticated deployment identity → database path/inode/snapshot identity → conversation/owner identity → API/SSE projection → persisted/logged transition. A mismatch anywhere in that chain can explain a false report without a runtime incident.

Normative authorities include `specs/bedrock/requirements.md` REQ-BED-006/007/020/021, `specs/bedrock/bedrock.allium`, `specs/llm*`, `specs/llm-retry-visibility/*`, and ADR-025. Read the versions at deployed d994 first, then current delivery-base versions. If overload behavior changes, update the current normative contract and add a superseding ADR when policy/rationale changes; do not rewrite ADR history.

## Required execution sequence

### 1. Freeze and identify evidence before mutation

Before `/trigger-continuation`, `/continue`, cancel, dismiss, retry, model change, message submission, runtime eviction, service restart, or another recovery mutation against #775/#789:

1. Preserve a read-only production SQLite snapshot plus file identity, authenticated API base/host, `/api/version`, service executable/process identity, deployed revision, boot/restart times, and relevant configuration identity. Query snapshots rather than live mutable state where possible.
2. Preserve narrow sanitized slices of active and rotated `prod.log`, TraceQL where retained, conversation/message rows, continuation states/messages/dispatch intents, `llm_request_metrics`, steering queues, durable turn/admission ownership, wake rows, runtime/process evidence, API responses, and prior coordinator tool receipts.
3. Reference and integrity-identify both coordinator snapshots (`...presnapshot-20260920T134642Z` and corrected `...correction-20260920T135246Z`) and Global's independent read-only evidence. Do not copy secrets or raw message content into Git.
4. Record exact SQL, log windows, trace queries, clocks/time zones, hashes, and negative-search bounds. Clearly label unavailable rotations/retention as missing.

### 2. Reconstruct observation provenance and owner timelines

First reconstruct how the three-hour claim was produced: the observing command/tool, host, API endpoint, authentication target, database file, deployment SHA, timestamp/time zone, conversation identifier mapping, cache/projection, and copied output. Compare that provenance with authenticated `devmbp` production identity. If the evidence shows an observation-source error, state it plainly as the incident RCA cause and explain why it was credible.

Then build independent #775 and #789 timelines from the earliest relevant persisted/logged event through their final known state. Include operation/request IDs, path type, typed provider outcome, attempts/delays/elapsed time, persistence, queue/admission/wake/runtime owner, process restarts/cancellation, SSE/API projection, successor status, and final transition. Tag every assertion **Observed**, **Inferred**, or **Missing**.

Search rotations, DB metrics, traces, and prior receipts before concluding there was no earlier `AwaitingContinuation`. A bounded absence conclusion must name all searched sources and retention gaps. Classify each owner as: confirmed runtime wait, stale/wrong projection, wrong machine/API/DB/source identity, queue or wake delay, provider request/backoff, owner shutdown/restart, or another evidenced cause.

### 3. Correlate overload separately

Build a separate #764/#777/#788 comparator table keyed by deployment SHA, timestamp, request ID, route/model, provider code, typed category, path type, attempt count, and final user-visible state. Begin with the confirmed #764/#777 one-attempt `server_overloaded` rows and the bounded absence of failed metrics for #788/#775/#789. Do not use those overloads to manufacture a cause for #775/#789.

### 4. State and assess the three retry contracts

For ordinary turns, continuation summary/compaction, and continuation opening, document at exact deployed d994 and current delivery base:

- transient and terminal classifications, especially overload versus rate limit/quota/auth/prompt rejection;
- provider-library retries versus Phoenix runtime retries;
- attempts, jitter/backoff delays, per-attempt timeout, and total elapsed deadline or its absence;
- whether queue/admission/runtime ownership remains held while requesting/backing off;
- exhausted-budget durable publication, UI state/action, and restart reconstruction;
- duplicate-result, successor, and opening-handoff fencing.

Assess a distinct overload policy that is bounded by elapsed time, uses jittered backoff, respects any provider retry hint, publishes attempts, and terminates visibly/actionably. Keep overload structurally separate from quota, auth, prompt rejection, and invalid requests. Do not automatically change model, service tier, or billing behavior. Do not increase the generic three-attempt budget blindly.

### 5. Reproduce two concerns independently

Use deterministic clocks and provider/runtime failpoints:

- **Observation-source reproduction:** prove how a host/API/DB/deployment/projection mismatch could yield the original claim while authenticated d994 production truth shows the corrected states; add diagnostics or evidence tooling only at the smallest owning identity boundary if needed.
- **Overload-policy reproduction:** inject exact `server_is_overloaded`/`slow_down` shapes into ordinary and continuation-summary requests. Prove deployed/current behavior is one terminal attempt, then exercise the proposed bounded jittered elapsed-budget behavior independently of #775/#789.

Do not call the overload policy the incident root cause unless new direct evidence ties it to an owner.

### 6. Deliver the smallest justified correction

Make the smallest structural change at the owning provider-policy/runtime/persistence boundary. The expected candidate is a typed overload-specific automatic retry policy with bounded elapsed budget and jittered delays, shared intentionally by ordinary and continuation-summary requests while leaving continuation opening on its single durable-intent/ordinary-admission authority. If assessment disproves that policy, document why and deliver only the smallest evidenced corrective control that still satisfies the commissioned capacity-handling requirement; do not invent a runtime incident.

Every admitted operation must converge to success or a durable visible actionable state within the policy bound, release or reconstruct ownership across cancellation/crash/restart, and preserve stable operation/message identity. No uncontrolled loop, model substitution, billing/service-tier action, scheduler framework, parallel lifecycle authority, broad queue rewrite, or unrelated retry-policy cleanup.

## Required regressions

- `server_is_overloaded` and `slow_down` follow the new overload-specific bounded elapsed budget with deterministic jitter/backoff assertions; quota, auth, prompt rejection, invalid request, and usage limits remain excluded.
- Ordinary-turn and continuation-summary overload each recover when capacity clears inside the bound, with visible attempt data and stable continuation operation identity.
- Exhausted overload budget publishes a durable visible actionable ordinary error or recoverable continuation failure, never permanent busy.
- Cancellation/owner abort and crash/restart at dispatch, backoff, outcome, and commit boundaries cannot strand admission, durable-turn ownership, steering queue, continuation intent, wake ownership, or timer authority.
- Duplicate/late outcomes and repeated recovery/open requests create no duplicate summary, message, successor, continuation edge, or opening handoff.
- Observation-source tests/receipts distinguish a monitor/source identity error from actual persisted/runtime state and do not reinterpret #789's 13:46 ordinary chat as recovery.
- Existing eligible-error 3-attempt behavior remains unchanged unless an explicit normative change is justified.

## Sanitized RCA artifact

Commit a sanitized physical RCA artifact containing:

- immutable evidence manifest, deployment/API/DB/machine identities, clock basis, and query bounds;
- the original three-hour report, the correction, and an observation-provenance analysis without erasing either;
- side-by-side #775/#789 timelines labeled Observed/Inferred/Missing;
- #764/#777/#788 overload comparator table;
- exact deployed-d994 versus delivery-base contract and topology differences;
- separate conclusions for observation cause and overload-policy adequacy;
- deterministic before/after receipts for identity diagnosis and overload handling;
- ownership/restart and no-duplicate receipts, remaining risks, and unavailable evidence.

Never commit production DBs, credentials, raw secret-bearing logs/traces, or message bodies beyond minimal redacted hashes/metadata.

## Delivery and acceptance

- Work on one new incident branch in this approved managed worktree on `devmbp`; leave #775/#789 owners, WorkScopes, worktrees, and branches untouched.
- Re-verify roadmap issue #651 and hosted owner/PR facts after approval; network was unavailable during Explore. Report only this workstream and do not rewrite another owner's record.
- Run focused state-machine/runtime/database/provider tests, focused UI tests if presentation changes, spec/Allium pre-flight when touched, and `./dev.py check`.
- Commit and push one branch, open one PR, and carry it through hosted CI.
- Obtain fresh exact-head Codex review after the final push; resolve and repush until the reviewed SHA is current. Reach cursor exhaustion with zero actionable hosted review threads and record URLs/SHAs/CI receipts.
- Do not merge or deploy.
- Do not stop at a causal memo or source-ready patch: acceptance requires the corrected RCA, bounded corrective delivery, deterministic regressions, sanitized receipts, pushed PR, green hosted CI, and zero actionable exact-head findings.

## Explicit non-goals

- No takeover, continuation, successor, branch rewrite, or source-scope work for #775/#789.
- No mutation intended to “unstick” #775/#789 absent fresh evidence that recovery is required.
- No claim that the reported three-hour runtime event occurred without corroboration.
- No claim that #764/#777 overload caused #775/#789 behavior without direct evidence.
- No automatic model/service-tier/billing change, generic retry inflation, broad scheduler rewrite, merge, or deploy.
