# Implement the wake plane for bash and tmux terminal waits

Replace the prior wake-contract implementation plan with a provider-safe, durable wake plane for bash and tmux terminal handles. The implementation is complete only when both substrates work end to end; bash is the proving milestone and tmux is a required fast-follow within this same task.

This task supersedes `tasks/25002-p1-ready--implement-wake-contracts.md` and the implementation sequencing in `tasks/54006-p2-done--structured-subagent-turn-limit-outcomes.md`. It deliberately does not absorb `tasks/54007-p1-ready--nonblocking-subagent-wake-handles.md`: sub-agent wake integration remains follow-up work after the wake-plane core is proven. Revise related tasks as needed so there is one authoritative implementation plan.

## Product outcome

An agent that has nothing useful to do until a bash or Phoenix-managed tmux command finishes can register a bounded terminal wait and stop spending turns on polling. Phoenix acknowledges the registration through the ordinary tool round, leaves the conversation Idle and user-interruptible, durably observes the eventual outcome, and resumes the agent exactly when the conversation can safely accept another LLM turn.

The wake plane is an accountability and delivery layer, not a promise that every watched handle survives Phoenix restart.

## Settled design decisions

### Registration receipt and later runtime observation

`wait_until` is an ordinary tool call with an immediate structured result:

```text
assistant: wait_until(handle)
tool: { registered: true, contract_id, expires_at }
— persist the complete tool round; become Idle without another LLM call —
runtime observation: { wake_contract_id, handle, outcome }
— resume once the conversation can safely accept one —
```

The terminal outcome is not a delayed `tool_result` for the registration call. It is a typed Phoenix runtime/meta-user observation correlated by `contract_id`. This preserves provider-valid tool-use/result ordering even with multiple waits, user messages, continuation, and restart.

If an assistant response contains multiple tools, execute and persist the complete serial tool round normally. The presence of one or more successful `wait_until` registrations makes the post-checkpoint disposition Idle rather than immediately invoking the LLM again. Do not leave unmatched tool uses in history.

Registration against an already-terminal handle still produces the ordinary receipt. The terminal observation is resolved durably and enters the same wake inbox; it is not folded into a second representation inside the receipt.

### Durable inbox and coalesced resume

Persist every terminal observation immediately. Permit at most one LLM request at a time for a conversation.

- If the conversation is Idle, schedule an idempotent wake resume.
- If it is handling a user message, LLM request, tool round, cancellation, or another wake, retain the observation durably until the conversation next becomes Idle.
- One resume includes all unconsumed wake observations available at the request snapshot, in committed message order.
- An observation committed after that snapshot waits for the following Idle transition.
- Multiple observations must not create overlapping LLM requests or one-turn-per-event storms.
- User messages remain accepted normally while contracts are pending. Existing busy-state steering behavior remains authoritative while an actual turn is running.

Define crash recovery for both durable observations and resume scheduling. The durable guarantee is one terminal transition and one conversation observation per contract; scheduling and consumption must be idempotent across restart.

### Continuation transfers delivery obligations

When a conversation continues into a successor, transfer every pending wake contract and every unconsumed wake observation to the successor as part of the continuation boundary. This includes future sub-agent wake contracts.

Handle identity does not change during transfer. WorkScope determines whether a bash/tmux resource survives the boundary; it does not determine who inherits responsibility for an outstanding result. Sub-agent identity and ownership remain parent/child based in this task. Designing sub-agents as WorkScope-owned resources is separate future work because it also affects cleanup, sibling lifetime, one-writer rules, and hard-delete.

Continuation transfer and terminal resolution must be serialized transactionally so exactly one conversation is the delivery target.

### Lifecycle requires explicit cancellation

Archive, abandon, mark-merged, and hard-delete must return a clear conflict while pending wake contracts exist. The user cancels pending waits explicitly and retries the lifecycle action. Do not silently or implicitly cancel waits as a side effect of ordinary lifecycle operations.

Explicit cancellation:

- atomically transitions the contract to `Cancelled` and appends its durable runtime observation;
- updates status/UI immediately;
- does not schedule an LLM turn solely because the user cancelled the wait;
- allows lifecycle gates to proceed once no contracts remain pending.

The cancelled observation remains part of conversation context for a later user- or wake-triggered request. A separately named destructive force-delete behavior is out of scope.

### Busy state remains semantically precise

Do not make `ConvState::is_busy()` depend on SQLite or treat an Idle wake-pending conversation as runtime-busy. Introduce a database-aware lifecycle guard equivalent to:

```text
lifecycle_blocked = state.is_busy() OR has_pending_wake_contracts(conversation)
```

Re-check the predicate transactionally in destructive lifecycle paths so registration cannot race the gate. Chat acceptance continues to use conversation state; pending waits alone do not block user input.

## Normative spec revision before runtime implementation

Revise the relevant timeless specifications and ADR before implementing runtime behavior. At minimum:

- `specs/wake-contracts/requirements.md`
- `specs/wake-contracts/wake-contracts.allium`
- `specs/wake-contracts/executive.md`
- a new ADR superseding the delayed-synthetic-tool-result portions of ADR-006 rather than rewriting accepted history
- `specs/bedrock/requirements.md` and/or `bedrock.allium` where post-tool-round Idle disposition, durable runtime observations, dispatch serialization, or lifecycle guards cross the bedrock boundary
- bash and tmux specifications for handle-specific evidence and payloads
- sub-agent requirements/specs only as needed to remove claims that incorrectly place sub-agent implementation in this task and to preserve the later integration contract

The revised specifications must:

1. Replace delayed tool-result delivery with immediate registration receipt plus typed runtime observation.
2. Define the durable inbox, coalescing, request-snapshot boundary, and busy-arrival behavior.
3. Define transactional exactly-once terminalization and idempotent delivery/resume semantics.
4. Transfer delivery obligations on continuation independently of handle resource ownership.
5. Replace the proposed `is_busy()` mutation with the aggregate lifecycle guard.
6. Define explicit cancellation without automatic LLM resume.
7. Narrow the implemented handle scope to bash and tmux while keeping sub-agent integration an explicit follow-up.
8. Add real Allium surfaces/triggers for tool registration, router evaluation, cancellation, startup reconciliation, continuation, lifecycle checks, and dispatch; eliminate unreachable-trigger warnings relevant to the feature.
9. Make invalid payload combinations structurally unrepresentable.
10. Remove rollout/status language from timeless requirements and Allium; keep implementation state in executive docs and tasks.

Run the pre-flight checklist in `specs/AUTHORING.md`, validate the full Allium dependency set, and update ADR/spec indexes and cross-spec anchors.

## Persistence model

Use normalized, queryable columns for v1 structure. Do not persist redundant representations of the same semantic value.

A contract needs at least:

- stable contract id;
- current delivery conversation id;
- handle kind;
- handle id;
- registration and expiry timestamps;
- status;
- terminal cause;
- finite forgotten reason when applicable;
- cause-specific terminal payload when applicable;
- resolution timestamp;
- any explicit durable dispatch/consumption state required by the inbox protocol.

Do not add `condition_json` for the sole v1 `HandleTerminal` condition: `handle_kind` and `handle_id` already represent it. Do not add `fire_template_json`; the previous plan named it without a distinct consumer or contract. If future condition kinds require more fields, add a queryable condition discriminator and normalize their addressable data when that need exists.

The polymorphic, cause-specific terminal body may be an earned aggregate only if it is always read/written whole and never queried by JSON path. It must not repeat terminal cause, forgotten reason, handle kind, or handle identity. Persist bash/tmux captured tail lines as normalized child rows keyed by `(contract_id, ordinal)`; absence is an empty tail.

One database transaction must:

1. claim a pending contract with a guarded transition;
2. record terminal discriminator/body and resolution time;
3. write all captured tail rows;
4. append the deterministic, idempotent conversation observation;
5. record any durable inbox state needed for later resume.

A unique observation/message identity derived from `contract_id` must prevent duplicate delivery. External SSE and runtime scheduling occur only after commit and must be recoverable from durable state.

Startup reconciliation completes before normal router evaluation/serving can race it. For each pending contract, one transactionally stable decision wins: in-deadline durable terminal evidence, forgotten handle, expiry, or re-registration.

## Deadline and evidence rules

Require `1 <= max_wait_seconds <= 1800`; default to 600 seconds. `expires_at` is the bounded delivery deadline, not the handle lifetime.

Define the authoritative evidence timestamp per substrate and use the same persisted timestamp in live routing and restart reconciliation:

- durable evidence recorded at or before the deadline wins over a later router tick;
- evidence first observed after the deadline must not be retroactively treated as in-deadline unless its source carries a trustworthy persisted occurrence time;
- expiry applies only when the handle remains evaluable and no qualifying terminal evidence exists;
- `Forgotten` means the terminal answer has become unknowable, not merely temporarily unavailable.

Use one documented equality rule at the boundary (`<= expires_at` or `< expires_at`) across requirements, Allium, schema tests, and implementation.

## Handle authorization

Knowing an id is insufficient.

- Bash/tmux registration is authorized only when the registering conversation resolves to the handle's owning WorkScope.
- Continuation transfer changes delivery ownership but not handle identity or substrate ownership.
- Reject unknown, malformed, wrong-kind, and cross-scope handles before accepting a contract.
- Use tagged handle types rather than flat option fields or unchecked strings wherever the boundary permits.

## Gated implementation milestones

### Milestone 1 — revised protocol and specifications

Resolve all blocking design contradictions above. Add behavioral tests/fixtures that demonstrate provider-valid registration history, durable inbox ordering, continuation transfer, lifecycle rejection, cancellation without auto-resume, restart recovery, and exactly-once terminalization.

No runtime implementation should be considered authoritative while it contradicts the revised normative artifacts.

### Milestone 2 — wake-plane core plus bash

Implement:

- normalized migrations and storage types;
- typed `wait_until` interception/receipt and post-tool-round Idle disposition;
- durable router/inbox and coalesced dispatcher integration;
- bash ownership validation and terminal evaluator;
- bash synchronous-wait-equivalent terminal payload and normalized tail capture;
- bash restart `Forgotten` behavior when terminal evidence is unknowable;
- continuation transfer;
- explicit cancellation;
- lifecycle aggregate guard;
- restart reconciliation and idempotent scheduling;
- status API sufficient to test the end-to-end flow.

Milestone 2 must be independently shippable and tested, but does not complete this task.

### Milestone 3 — required tmux fast-follow

Before implementation, align the tmux spec and registry around durable terminal evidence. Implement:

- authorization by WorkScope and stable `window_id`;
- detection of the Phoenix `tmux_run` exit marker without requiring the inspectable window to close;
- explicit killed-window terminal evidence;
- authoritative terminal evidence timestamps;
- durable evidence sufficient for restart reconciliation when known;
- missing session/window `Forgotten` only when no durable terminal evidence exists;
- terminal payload, exit information when available, and normalized final tail;
- continuation and teardown behavior consistent with WorkScope ownership.

This task is not done until tmux waits work end to end.

### Milestone 4 — complete user and operator surfaces

Implement and verify:

- inline conversation wake indicator with pending count/list and soonest expiry;
- per-contract explicit cancellation;
- actionable lifecycle conflict showing pending contracts and how to cancel them;
- SSE/API status updates derived from durable contract state;
- `phoenix-client.py wake-status` with contract ids, handle kinds/ids, expiry, status, terminal cause, and forgotten reason;
- observability for registrations, resolution latency, causes, forgotten reasons, queued/coalesced resumes, and recovery/idempotency conflicts.

Keep this separate from WorkScope resource inventory UI; wake status is conversation delivery state, not a list of owned resources.

## Required concurrency and recovery tests

Cover at least:

- complete provider-valid receipt round with no unmatched tool use;
- several `wait_until` calls in one assistant response;
- `wait_until` mixed with ordinary serial tools in either order;
- already-terminal handle registration;
- two contracts resolving in one router pass and producing one coalesced resume;
- outcome arrival during LLM request, tool execution, cancellation, and request-snapshot construction;
- user message racing wake resolution;
- cancellation racing fire/expiry;
- continuation transfer racing resolution;
- lifecycle guard racing registration;
- duplicate router workers/ticks where only one guarded terminal transition wins;
- crash before terminal transaction, after terminal transaction but before scheduling, and after scheduling but before consumption acknowledgement;
- startup precedence among durable evidence, forgotten, expiry, and re-registration;
- tail rows and parent terminal/message state remaining atomic;
- bash handle loss on restart;
- tmux survival, durable terminal evidence, and missing-window cases;
- no automatic LLM resume for explicit cancellation;
- cancelled observations appearing in a later naturally-triggered context;
- no overlapping LLM requests and no one-turn-per-event storm.

## Verification

Use `./dev.py` for development and checks. At minimum run:

- focused Rust storage/router/runtime/API tests throughout each milestone;
- full Allium validation for the declared dependency set;
- spec anchor and spec-shape checks;
- generated SSE type/codegen checks when wire types change;
- CLI tests for `wake-status`;
- UI tests for status, cancellation, and lifecycle conflict;
- `./dev.py tasks validate`;
- `./dev.py check` before completing the task.

Commit completed milestones as separate logical units. Do not mark this task done after bash alone.

## Explicit non-goals and follow-ups

- non-blocking sub-agent spawn and first-class child handle exposure;
- sub-agent wake evaluator and terminal-cause persistence changes;
- making sub-agents WorkScope-owned resources;
- parent-to-child continuation, clarification, or budget extension;
- general conversation actors or request/reply messaging;
- compound `wait_any` / `wait_all` semantics;
- webhook, file, regex, port, browser, or deadline-only conditions;
- silently cancelling waits during lifecycle actions;
- a first-class `AwaitingWake` conversation state;
- force-delete with pending waits.

When sub-agent wake integration is designed, its pending delivery obligations transfer to a continuation successor under the same rule as bash/tmux, while child handle identity remains unchanged. WorkScope ownership for sub-agents requires its own cross-spec decision rather than being smuggled into wake delivery.


## Umbrella authority and dependency

Task 47003 is the sole authority for shared-engine ownership, sequencing, migration gates, and release criteria. This task preserves its historical design context and narrower acceptance detail, but any conflicting creation-first, wake-only, bespoke-scheduler, or rollout direction is superseded. Implementations SHALL follow `specs/durable-workflows/requirements.md` and ADR-010 through ADR-012.

Completion is redefined as engine-backed wake adoption under task 47003; the existing wake implementation is a normative profile input, not a permanent independent scheduler.
