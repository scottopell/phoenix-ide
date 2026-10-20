# ADR-186: Per-goal logical request admission and genuine typed Start

- **Status:** Proposed
- **Date:** 2026-10-20
- **Affects:** REQ-GOAL-002–005/009/015–018; goal execution state, canonical Start admission, summary handoff, request accounting and dispatch

## Context

At this decision point, exact `fab412e9a8a59187cade13a96bf83751b4c81843`
qualified bounded local typed-watch behavior in run `39482419906`: expanded
233/233, UI 12/12, compile, codegen and generated freshness passed. Broad tests
still timed out and shared Clippy remained red. Goal consumers were default OFF,
production services were uninstalled, and automatic follow-through was outside
the normative contract. This proposal does not activate execution or reinterpret
that evidence as Start/follow-through qualification.

The primary user then locked the finite-work policy: **each immutable goal has
its own finite limit; count new model requests including the initial request,
with or without tools; pause/restart/summary preserve usage; retries/replays do
not double-charge; reaching the limit ends the goal; a deliberate new goal gets
a new limit; Stop remains authoritative.** Numeric calibration is separate and
must be explicitly supplied, with no production default. Older assistant
proposals about cross-goal lifetime/commission allowances are not user authority
and are not adopted here.

The code had useful boundaries but not this feature. `GoalCommandService`
accepted Create/State mutations; `GoalManagementService` authenticated existing
Coordinator/owner principals. `settled_active_transcript` rejected competing work,
waits, inactive/Close and reservations. Direct-turn admission persisted existing
immutable goal/NoGoal observations. The ordinary no-tool `LlmCompleted` transition
went to Idle; `ConversationRuntime::process_event` drained accepted steering
before terminal settlement. `GoalDispatchPermit` fenced actual provider sends
against Stop. These seams must be extended, not mistaken for a working Start or
accounting implementation.

Normal `request_llm` generated a request UUID per invocation, except an overload
retry reused its logical identity. `LlmRequesting` persisted only an attempt.
Summary requests used continuation operation identity. Neither process-local
UUID generation nor best-effort usage telemetry established one durable,
recoverable per-goal logical-request admission across both paths.

## Decision

### Policy and ownership

Adopt the locked per-immutable-goal request policy in REQ-GOAL-015–018 and
`goal-execution.allium`. The execution installer must provide a positive finite
ceiling explicitly. Installation remains separate from watch installation and
schema application; uninstalled paths add no request-accounting/follow-through
goal SQL, Start capability or automatic work. Existing resource/tool-cycle caps
remain constraints, not a goal allowance or permission to raise resources.

A goal's finite eligibility/ceiling belongs to that goal identity. Usage derives
from its unique admitted logical requests. A new goal gets its own limit; no
shared commission identity, cross-goal budget, lifetime statistics requirement,
reset command or independently writable used counter is introduced. Pre-policy
goals without eligibility refuse goal-bound execution explicitly; no historical
ledger or numeric default is fabricated.

The candidate persistence shape is a per-goal eligibility/limit record plus
normalized request admissions associated with the existing execution/workflow.
This is **not DDL approval**: no table names, key layout, sequence/ordinal scheme,
migration number or generated shape is allocated by this draft. Detailed type
and schema review must keep identity, binding, capacity and state atomic rather
than introduce parallel accounting representations.

### Typed Start is canonical input, not a chat shortcut

Create/State mutate goals; release changes only the independent Stop fence.
Neither emits Start. An authenticated user or verified existing Coordinator
requests Start as a distinct typed action, serialized through existing product
and acceptance admission, at the actual settled boundary. It must not compete
with an active/accepted turn, queue, structured wait, Close or continuation/
creation reservation. Coordinator management does not create another actor or
extend resource/merge/deploy authority.

The persisted canonical Start input must carry its exact request identity,
authenticated requester, owner, expected goal/version, admitted execution/
workflow identity and immutable observation. Receipt, canonical host input and
owning turn admission commit together. Runtime materialization/recovery must
consume this typed case; existing chat payload code does not implement it merely
because a service can set `InputOrigin` or request a model call.

Replay first derives and authorizes the actual caller for the target, before
receipt lookup. It then compares the verified requester with the stored requester
and every typed command field: request identity, owner, immutable goal, expected
goal/version and Start command. Same-target authorization or an opaque string's
equality is not an exact requester/input check. A changed requester or field at
the same identity conflicts. An exact historical replay returns the same receipt
without fresh settled, Stop, current-goal or current-version admission; returning
a receipt is not provider dispatch or permission to recreate accepted work.
A receipt cannot revive a revoked execution or refill the goal. Initial request
accounting follows admission, using the same durable request mechanism as later
requests. Stored Start input is not a transcript UserChat, assistant/tool message,
`display_data` or persisted `llm_text`. Model context projects admitted facts
request-locally before budgeting/freezing. Shared transcript serialization must
not leak those facts; required-input/decoder/context failures refuse execution.
No concrete HTTP/SSE shape or new public endpoint is claimed by this draft.

### One durable request identity, with explicit crash cuts

Extend the existing logical-request identity concept across normal requests,
summary requests and restored dispatch. Do not add a second goal-only random
counter or treat each transport attempt as new work.

The immutable intent is the causal slot, purpose and observed execution/goal
binding, not a frozen provider prompt. Accepted input may change attempt-local
context: drain it, rebuild/budget/freeze the projection and revalidate a shared
send grant without minting another intent or charge. Non-resetting accepted and
incorporated revisions distinguish a genuinely incorporated queue from an empty
queue ABA. The first externally acting provider poll is serialized with message
acceptance, live claim invalidation and Stop. Preserve database-writer-before-
epoch ordering, release short gates on Pending and retain cancellation through
the existing dispatch epoch; neither a mutex held for the whole stream nor an
unguarded reusable send closure implements this boundary.

| Cut | Required durable result and recovery |
|---|---|
| Before first request admission commits | No charge or admitted state; no provider dispatch. Retrying planning may establish one intent only under fresh authority. |
| Admission commits before provider dispatch | Immutable identity, canonical intent, exact binding, one admission and next request state survive together. Restore this same intent; recheck Stop/lifecycle/authority before dispatch. |
| Provider request/response interrupted before response checkpoint | Recovery retains the admitted identity and charge, including overload/transport retry. No free replan, duplicate debit or provider-side exactly-once claim. |
| Response/tool/report checkpoint commits before host decision | Restore the committed result and decision phase, not another dispatch of the completed logical request. |
| Next-request or settle decision commits before effect completion | Restore that exact admitted next request or settlement, using the existing owning checkpoint protocol; never refund/reset the goal or publish an uncommitted result. |
| Summary request/handoff interrupted | Same summary intent is charged once. Successful same-product handoff preserves goal binding/history/usage; every new successor model request is a new admission, including its first request. |

The first-request identity must be persisted at the admission/next-state boundary,
not generated only after a request becomes externally dispatchable. Both normal
and summary paths call the same accounting authority before provider freeze and
spawn. Identity reuse with changed purpose, owner, goal, execution or canonical
intent is a conflict. Storage/decode/constraint/commit failures remain errors, not
NoGoal, unlimited capacity, successful Start or silent ordinary execution.

This specifies recovery for this new durable request intent only. It grants no
cross-version/downgrade/rollback guarantee, provider-side exactly-once guarantee
or general timestamp policy beyond the compatibility requirements.

### Last permitted response wins before capacity exhaustion

Admission of the last allowed request is not immediate goal exhaustion. Its
actual response, legitimate tools and valid bound report finish their checkpoint
first. At the next semantic request/settle decision, only a still-current,
still-Underway bound goal with no remaining capacity becomes host-derived
Exhausted. A committed Complete, Blocked or other non-Underway result is preserved.
Exhaustion and settlement commit before the causal terminal watch capture; no
extra grace request is allowed.

Introduce a narrow durable host-decision phase before ordinary Idle/terminal
settlement. Existing accepted steering/input drains first, retaining queue
ownership. Its presence cannot bypass finite request accounting. The decision
revalidates root/latest/open state, immutable binding, current goal, scope,
Stop/control fence and existing execution authority. If it remains Underway and
has capacity, same-execution follow-through requests another admitted logical
call even after a no-tool yield. If blocked/suspended/complete/exhausted,
observed-NoGoal, stopped, inactive/closed, stale or superseded, it does not grant
automatic work. Structured waits remain execution suspension, not automatic
goal-status coupling. Dispatch still consumes existing live Stop/execution
permission after any committed decision. Independently, accepted queue presence
must be checked at the actual serialized dispatch admission: the Stop permit is
not a queue predicate. If input arrives after request admission while its phase
is requesting, defer/refuse the send/retry and drain accepted input through
existing queue/checkpoint ownership. Retain the same immutable admitted intent
and charge, without refund/refill, replan, Idle, new turn or synthetic Start.
After draining, recheck eligibility for that same intent under current authority;
no cached Boolean decision can justify overtaking accepted input.

### Delivery sequence and non-goals

A first implementation slice may establish accounting plus genuine typed Start,
then add natural-yield host decisions. That is engineering order, not removal of
Start or permission to activate a partially closed feature. User Start and
verified Coordinator Start, both request paths, recovery, queue/Stop races,
privacy, default-OFF inertness and final-request ordering remain full closure
obligations. No new scheduler/worker, watch-triggered execution, ambient goal
lookup, synthetic chat, grace mode, autonomous management principal or numeric
default is authorized.

## Alternatives considered

1. **Continue after model yield using synthetic UserChat or a worker.** Rejected:
   changes message provenance/authority, invents a second owner and bypasses the
   real queue/Stop/admission pipeline.
2. **Only charge automatic/no-tool advances.** Rejected by the user policy:
   initial, tool-bearing and goal-bound summary/successor calls count too.
3. **Shared commission/lifetime allowance with reset restrictions.** Rejected:
   the primary user chose per-immutable-goal scope and new limits for deliberate
   new goals; earlier assistant proposals are not authority.
4. **Use transport UUIDs or telemetry totals as the ledger.** Rejected: replay,
   retry and crash cuts do not align with telemetry/process identity lifetimes.
5. **Exhaust on admitting the final request or add a grace call.** Rejected:
   premature exhaustion fences the allowed final report; grace exceeds the limit.

## Consequences

- **Positive:** finite goal work is explainable and durable across providers,
  tool/no-tool responses, retries and transcript succession without a new actor.
- **Positive:** typed Start and request-local context preserve provenance and
  shared-transcript privacy; Stop and existing admission remain authoritative.
- **Negative:** normal and summary request state, canonical input materialization,
  checkpoint/recovery, provider dispatch and host settlement all need coordinated
  typed changes. A superficial counter or Idle hook is insufficient.
- **Negative:** pre-policy goals without explicit eligibility cannot silently
  execute. Installation needs an authorized numeric value and a qualified path;
  the docs draft supplies neither installation nor execution evidence.
- **Neutral:** per-goal row/admission records are a candidate, not a DDL lock.
  Migration allocation, physical schema/types and API presentation require review
  in their own implementation packet. Migrations 132–135 remain immutable.

## References

- [Goal requirements](../conversation-goals/requirements.md), REQ-GOAL-015–018.
- [Goal execution lifecycle](../conversation-goals/goal-execution.allium).
- [ADR-185](185_goal-control-installation-is-an-explicit-owner-policy.md).
- [ADR-184](184_local-goal-watch-facts-belong-to-their-causal-event.md).
- [Compatibility requirements](../compatibility/requirements.md), ADR-034.
- Existing seams: `GoalCommandService`, `GoalManagementService`,
  `settled_active_transcript`, `capture_goal_at_new_turn_tx`,
  `ConversationRuntime::process_event`, `terminalize_direct_turn_if_done`,
  `request_llm`, `request_continuation`, `GoalDispatchPermit`.
