# ADR-059: Selected-model overload uses a durable bounded retry window

- **Status:** Accepted
- **Date:** 2026-09-20
- **Affects:** REQ-LLM-006, REQ-LLM-006b, REQ-LRV-002, REQ-LRV-005, REQ-LRV-008, REQ-BED-006, REQ-BED-007
- **Supersedes:** The terminal-overload aspect of the LLM error-taxonomy decision embodied in `specs/llm/requirements.md` before REQ-LLM-006b
- **Qualifies:** ADR-025, ADR-048

## Context

Phoenix classified model-capacity responses separately from generic server errors, but treated them as terminal. That avoided wasting the generic retry budget and avoided silently changing the selected model, yet it made short-lived provider congestion immediately user-visible as failure.

A capacity retry cannot simply join the generic transient loop. Capacity incidents benefit from slower backoff, jitter, provider hints, and a wall-clock bound that survives process restart. Continuation-summary generation also has a durable operation identity that must not be lost or duplicated when capacity retry stops.

ADR-048 established an absolute deadline for each provider attempt. That bounds one network call, not a multi-attempt capacity incident. ADR-025 established idempotent durable continuation operations. Capacity retry must preserve that operation rather than create a parallel continuation lifecycle.

## Options considered

1. Keep model-capacity errors terminal: simple, but exposes brief congestion as immediate failure.
2. Fold capacity into generic retry: reuses machinery, but gives the wrong budget and does not define restart-stable jitter or an incident deadline.
3. Use a dedicated durable same-model policy: preserves taxonomy and user routing while bounding attempts, time, and recovery behavior.

## Decision

`ServerOverloaded` remains distinct from quota exhaustion, authentication failure, prompt rejection, invalid request, and generic server error. It enters a dedicated same-model retry policy rather than terminating immediately or inheriting the generic retry policy.

The policy permits 5 total provider attempts. Waits before attempts 2 through 5 have nominal values of 4, 8, 16, and 32 seconds. Each wait receives deterministic jitter in the inclusive range ±25%, derived from stable logical request identity and target attempt. The same logical retry therefore computes the same jitter after restart.

A valid provider `Retry-After` hint of at most 30 seconds is a floor for the jittered wait. A hint over 30 seconds stops automatic retry visibly; Phoenix does not truncate it and pretend the provider recommended a shorter wait.

An absolute 120-second incident window begins when the first overload is observed. Later overloads and process restarts do not renew it. A retry whose computed time reaches or exceeds that deadline is not scheduled.

The retry state durably records its logical target, target attempt, waiting retry time, first-overload time, deadline, and waiting or in-flight phase. Startup rearms one future timer, dispatches one due retry, redispatches one persisted in-flight target, or expires without provider dispatch. Cancel and Close retire timer and provider-admission authority so stale outcomes cannot regain ownership.

Ordinary-turn exhaustion enters terminal `Error`. Continuation-summary exhaustion enters `RecoverableContinuationFailure` while retaining the continuation operation identity and inputs. Opening automatic continuation does not introduce a second capacity loop; the request has one overload loop across that boundary.

The generic transient policy remains 3 total attempts with nominal waits of 2 and 4 seconds.

Retry visibility identifies `server_overloaded` and carries target attempt, maximum attempts, and remaining countdown. Stopping conditions replace the countdown with the target-appropriate visible failure.

## Relationship to earlier decisions

This decision supersedes only the terminal-overload behavior of the prior LLM taxonomy. The distinct `ServerOverloaded` classification remains in force.

ADR-048 still governs the absolute deadline of each individual provider attempt. Its deadline does not renew or replace the 120-second overload-incident deadline; both limits apply independently, and either may stop work.

ADR-025 still governs continuation compaction as one idempotent durable operation. The overload state is retry metadata for that operation, not a second operation identity or commit authority. Recoverable continuation failure retains the original operation for explicit retry.

## Consequences

Short-lived capacity incidents can recover without changing the user's selected model. Deterministic jitter reduces synchronized retries while preserving restart behavior. Durable retry phase and an absolute incident window prevent restart from resetting the budget or silently multiplying dispatches.

The state model and wire projection are more explicit than the generic retry loop: capacity waiting and dispatch must persist enough timing and identity to recover exactly once, and UI clients must render a countdown. Long provider hints stop sooner from Phoenix's perspective, but they remain visible instead of being misrepresented as a capped wait.

## References

- `specs/llm/requirements.md` — REQ-LLM-006, REQ-LLM-006b
- `specs/llm/llm.allium` — selected-model overload rules and invariants
- `specs/llm-retry-visibility/requirements.md` — REQ-LRV-002, REQ-LRV-005, REQ-LRV-008
- `specs/bedrock/requirements.md` — REQ-BED-006, REQ-BED-007
- `specs/adrs/025_continuation-compaction-is-an-idempotent-durable-operation.md`
- `specs/adrs/048_absolute-llm-provider-attempt-deadline.md`
