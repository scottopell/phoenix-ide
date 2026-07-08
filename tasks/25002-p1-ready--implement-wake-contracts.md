# Implement wake contracts v1

Implement the wake-contract runtime described by `specs/wake-contracts/` and ADR-006. This is the central tracker for making async terminal waits real in Phoenix.

## Current spec authority

- Requirements: `specs/wake-contracts/requirements.md` (`REQ-WAKE-001` through `REQ-WAKE-018`)
- Behavioral model: `specs/wake-contracts/wake-contracts.allium`
- Status/current reality: `specs/wake-contracts/executive.md`
- ADR: `specs/adrs/006_wake-contracts-are-persisted-conversation-scoped-terminal-waits.md`

Treat wake contracts as durable wait intent and a delivery obligation, not as durable process handles. Accepted contracts resolve exactly once while their current conversation remains queryable: `Fired`, `Expired`, `Cancelled`, or `Forgotten`.

## V1 scope

Implement concrete `HandleTerminal` waits for:

- bash handles;
- tmux `window_id` handles returned by `tmux_run`;
- sub-agent child conversation / agent ids that already exist and are waitable.

Do not implement a general actor framework, cross-conversation wake registration, webhook wakes, deadline-only wakes, compound `wait_all`/`wait_any` semantics, or a first-class `AwaitingWake` conversation state.

Non-blocking sub-agent spawning / parent-facing child handle exposure is tracked separately in `tasks/54007-p1-ready--nonblocking-subagent-wake-handles.md`. This task may support waiting on already-known child ids and may lower existing blocking `spawn_agents` fan-in internally, but it should not expand the spawn surface beyond the current spec without coordinating that task.

## Required implementation surfaces

### Persistence and schema

Add a normalized wake-contract persistence model with queryable discriminator columns, including at least:

- `id`
- `conv_id`
- `handle_kind`
- `handle_id`
- `condition_json`
- `expires_at`
- `registered_at`
- `fire_template_json`
- `registering_tool_use_id`
- `status`
- `terminal_cause`
- `forgotten_reason`
- `terminal_payload`
- `resolved_at`

`terminal_cause` and `forgotten_reason` are columns because metrics/operator views group on them. `terminal_payload` is only the cause-specific body and must not repeat those discriminator values.

### `wait_until` tool

Add the `wait_until` tool with a tagged handle discriminator and v1 `HandleTerminal` condition.

Registration must:

- validate handle ownership / reachability for the registering conversation;
- persist the contract durably;
- return an immediate tool result or otherwise keep the original assistant `tool_use` paired so later LLM requests are provider-valid;
- leave the conversation in `Idle` rather than adding an `AwaitingWake` state.

### Wake router and delivery

Implement a wake router that evaluates pending contracts and resolves each accepted contract exactly once.

Delivery must:

- append the synthetic result to the contract's current `conv_id`;
- pair the original `registering_tool_use_id`;
- trigger the next LLM turn when appropriate;
- emit UI-visible wake status changes;
- preserve delivery across restarts when terminal evidence was durable.

Hard-delete is the exception: cancel/remove contracts before deleting the conversation row and do not append synthetic results into deleted conversations.

### Terminal semantics

Bash fired payloads are terminal-only and mirror the synchronous bash wait surface. Missing/lost bash handles resolve top-level `Forgotten`, not a fired bash payload.

Tmux fired payloads fire on Phoenix `tmux_run` exit marker observation or recorded killed-window terminal state. A missing tmux window/session with no recorded terminal state resolves top-level `Forgotten`.

Sub-agent fired payloads must preserve the durable child terminal cause taxonomy required by `REQ-WAKE-017`, including explicit submissions, timeout, independently observed child cancellation, turn-limit fallback, implicit text completion, runtime failures, and context exhaustion. Missing child handles resolve top-level `Forgotten`.

### Restart reconciliation

On startup, reconcile pending contracts before normal serving using the spec ordering:

1. in-deadline durable terminal evidence fires;
2. handles that became unknowable resolve `Forgotten`;
3. overdue evaluable contracts without in-deadline terminal evidence expire;
4. still-pending durable handles re-register with the router.

### Cancellation and lifecycle

Implement explicit cancellation and lifecycle cascade behavior. User/lifecycle cancellation resolves top-level `Cancelled`; destroyed or unknowable handles resolve `Forgotten` only when the conversation remains queryable.

### Status, CLI, and UI

Implement wake-status visibility:

- UI indicator for pending wake contracts and cancellation;
- `phoenix-client.py wake-status` exposing pending count, soonest expiry, per-contract ids, handle kinds, and terminal status.

### Metrics

Emit observability for:

- registration rate;
- fire latency;
- expired-vs-fired ratio;
- forgotten-vs-fired ratio broken out by finite reason (`phoenix_restart`, `cascade_destroyed_handle`, `subagent_handle_missing`, `tmux_handle_missing`).

## Verification checklist

- Unit tests for schema transitions and invalid terminal payload shapes.
- Router tests for `Fired`, `Expired`, `Cancelled`, and `Forgotten` for each v1 handle kind.
- Restart reconciliation tests covering in-deadline terminal evidence, missing handles, overdue contracts, and re-registration.
- Conversation-history tests proving the original `wait_until` tool use remains provider-valid and paired.
- UI/API/CLI tests for pending status and cancellation.
- Metrics tests or assertions for discriminator columns and forgotten reason buckets.
- `./dev.py tasks validate`
- `./dev.py check --lanes rust,allium,spec-anchors,spec-shape`

## Out of scope / follow-ups

- First-class non-blocking sub-agent spawn/handle exposure: `tasks/54007-p1-ready--nonblocking-subagent-wake-handles.md`.
- Deadline-only / wall-clock wakes such as usage-limit reset sweeps. If added later, they need their own condition kind and cannot reuse handle-ownership or `Forgotten` semantics.
- Browser handles or any non-terminal handle kind.
