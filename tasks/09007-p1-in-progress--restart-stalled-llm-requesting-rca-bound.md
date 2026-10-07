# Restart-stalled LlmRequesting RCA and bounded structural fix

Own the restart incident RCA/fix only, on the managed `devmbp` worktree and branch created from exact primary `83843934130256cfd9deab9a5bb30ca53d578dd3`. Preserve owner `20385`, WorkScope `8f03`, existing implementation/review work, and the separate #797 selected-model-overload scope. Do not revive/bypass closed owner `0c5`, create a successor, or silently roll unfinished #797 work into this delivery.

## Incident observation to reconstruct

Primary coordinator `b892999b` with provenance `ef4` remained `LlmRequesting` from 2026-09-29 20:48:49 through 2026-09-30 00:40 (>3h46m), with no provider dispatch/results observed. At 20:48:49.034 `shutdown_kill_tree` killed three bash process groups; interrupted results persisted `LlmRequesting` plus "Making LLM request" at 20:48:49.090/.111. Old shutdown completed 20:48:51; new startup began 20:48:54.318. Recovery's `has_persisted_llm_request_owner` preserved `LlmRequesting`; `needs_auto_continue=false`. Coordinator resumed only after steering. Global applied no cancel/retry. A provenance-guarded cancel plus one chat at 00:41, followed by real tools at 00:41:46, is a workaround receipt, not proof of cause or fix.

Exact evidence is expected in coordinator worktree `.phoenix/evidence/restart-stall-20260930/events.json` and `startup-events.json`; accept bounded forwarded bytes, hash/sanitize them, and clearly mark unavailable evidence. Reconstruct source/deployment/machine/DB identity before conclusions.

## Required investigation

Trace the owning predicates and lifecycle across steering, approval, creation, startup hydration/materialization, admission/direct-turn owner, dispatch, provider request, shutdown quiescing and process-tree kill. Determine whether shutdown launched new provider work, startup preserved a state without reconstructing its dispatch obligation, an undispatched request was mistaken for an owned request, or another evidenced cause. Distinguish no-spawn from a hung provider request. Account for existing 10-minute HTTP attempt and 30-second WebSocket frame guards, which cannot govern a request that was never dispatched. Investigate the coordinator aggregate returning 404/closed history while routed transcript remained working as an identity/projection lead; do not patch the DB or assume it is causal.

Every RCA assertion must be Observed, Inferred, or Missing. Preserve read-only SQLite/log/TraceQL/process/runtime evidence before any incident recovery mutation. No live crash experiment, deployment, production DB mutation, cleanup, cancel, retry, or service restart.

## Required correction

At the smallest owning runtime/persistence boundary, ensure a persisted admitted LLM intent interrupted by process restart converges without a new user message to exactly one of:

1. the same logical intent is reconstructed and dispatched/resumed once under its stable ownership/identity; or
2. a durable visible actionable terminal error is published and admission/queue ownership is released.

It must never remain phantom-busy indefinitely. Shutdown quiescing must prevent launching new provider work after close begins. Startup must distinguish durable intent ownership from stale display state and must not duplicate provider attempts, assistant results, tools, summaries, or successors. Do not add approval menus, model/account fallback, broad fan-out, scheduler framework, or unrelated lifecycle rewrite.

## Required qualification

- Deterministically reproduce the restart cut at pre-dispatch, dispatch-admission, provider-in-flight, and result-persistence boundaries.
- Prove browser-free startup resumes the same intent once or terminalizes visibly without a user message.
- Prove cancel/Close releases admission and stale/late outcomes cannot duplicate publication.
- Prove HTTP/WS timeout/backoff/error paths remain owned after actual dispatch and are not used to mask undispatched work.
- Add exact source/deployed topology comparison, focused tests, `./dev.py check`, sanitized physical RCA/receipts, one pushed PR, hosted CI, fresh exact-head Codex review, and zero actionable threads.
- Global alone merges/deploys. No merge/deploy from this branch.
