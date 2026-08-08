# Deletion-first durable result subscriptions

## Status

Design snapshot only. Not approved for implementation. Expected to evolve through review.

PR #621 and its adapter stack were closed after the investigation showed that both the legacy wake system and the proposed wake-contract foundation create competing wake-specific authorities. Closing those PRs does not approve this replacement plan.

## Product contract under review

Preserve only:

- Enrollment is explicit through one tagged `wait_until`; ordinary resource creation never enrolls.
- Bash, TmuxWindow, and Subagent can produce typed terminal results without agent polling.
- One explicit wait produces one correlated durable result.
- A busy conversation stores the result without starting a competing turn.
- Cancelling the wait suppresses automatic continuation but does not kill the underlying resource or discard its eventual result.
- A Bash resource lost across Phoenix restart produces a typed restart-loss result.
- A Phoenix-managed tmux window may survive restart and be re-observed.
- Automatic continuation, if retained, uses only existing durable direct-turn/runtime admission.

Do not preserve deadlines, expiry, delivery-owner transfer, generic wake conditions, wake-specific admission, implicit enrollment, wake-specific UI lifecycle, or compatibility for unresolved legacy obligations unless later evidence proves one necessary.

## Proposed single-authority map

- Subscription intent and durable observation/delivery: existing generic workflow rows.
- Bash terminal truth: `BashHandleRegistry` / `Handle`.
- TmuxWindow terminal truth: `TmuxRegistry` plus the OS tmux process.
- Subagent terminal truth: existing conversation/subagent state machine.
- Correlated durable conversation message and exact replay: existing direct-turn materialization.
- Busy behavior and optional continuation: existing conversation admission and durable direct-turn admission.
- Transcript, SSE, and UI: derived projections only.

No wake-specific component may own resource lifecycle, conversation state, or runtime admission.

## Options reviewed

### A. Adapt the legacy wake system downward

Rejected as the likely end-state. It retains the legacy repository, worker, profile, recovery heuristics, and wake vocabulary while translating generic workflow facts into a second semantic system.

### B. Replace legacy wake with a smaller wake aggregate

Potentially correct, but rejected as a separate repository/authority. A tiny state description may remain useful for reasoning, but generic workflow attempts, observations, receipts, deliveries, and suppression already encode it.

### C. Typed durable result subscriptions

Current recommendation. This is one fixed profile with a tagged target and tagged result, not a generic pub/sub or provider framework.

Candidate target shape:

```text
WaitTarget:
  Bash(handle)
  TmuxWindow(server_token, window)
  Subagent(conversation)
```

Candidate result shape:

```text
WaitResult:
  BashExited(...)
  BashKilled(...)
  BashLostOnPhoenixRestart
  TmuxCompleted(...)
  TmuxGone(...)
  SubagentCompleted(...)
  SubagentFailed(...)
```

## Component disposition under review

### Delete

- `crates/phoenix-db/src/workflow/wake.rs`
- `crates/phoenix-ide/src/runtime/wake.rs` and `WakeWorker`
- `crates/phoenix-workflow/src/wake_profile.rs`
- `WakeRegistrar` and production wake registrar wiring
- Legacy wake-specific delivery-message and recovery-tail logic
- Wake-specific runtime admission and auto-resume heuristics
- Implicit tmux wake registration
- PR #621's `wake_contract` model, repository, and migration; the PR is closed and unmerged

### Retain as existing authorities

- Generic workflow attempts, observations, receipts, deliveries, runtime-acceptance status, and suppression
- Direct-turn durable message materialization and replay
- Conversation admission mutex / existing durable runtime admission
- Bash, tmux, and subagent lifecycle authorities

### Retain only as thin resource adapters

- Bash lifecycle event -> typed result observation
- Tmux lifecycle/reprobe -> typed result observation
- Subagent terminal transition -> typed result observation

### Derive as projections

- Transcript representation
- SSE/UI status
- Any inspection API

## Rough code-size hypothesis

Measured source sizes:

- Legacy DB wake implementation: 8,622 total LOC; production code ends near line 5,127.
- Legacy runtime wake implementation: 1,501 total LOC; production code ends near line 732.
- Legacy wake profile: 507 LOC.
- Unmerged #621 model: 1,542 LOC.
- Unmerged #621 repository: 2,688 total LOC, approximately 1,500 production LOC.

Hypothesis:

- Delete approximately 6,000-7,000 existing production LOC.
- Avoid merging approximately 3,000 additional production LOC from #621.
- Add approximately 900-1,500 production LOC for one fixed subscription profile and three thin adapters.
- Expected net reduction from current production: approximately 5,000 LOC.

These estimates require a source-level implementation plan before approval.

## Proposed hard-cut migration

The user explicitly permits dropping or manually cancelling unresolved legacy obligations. No compatibility bridge, dual write, dual read, or translation migration is required.

1. Freeze implicit registration and remove the remaining tmux implicit producer.
2. Cancel/drop any unresolved legacy obligations ad hoc before schema removal.
3. Delete the legacy wake runtime authority and wiring.
4. Delete legacy wake persistence/model/schema and recovery projections.
5. Add one narrow durable-result subscription profile using generic workflow primitives.
6. Add Bash durable-result delivery with automatic continuation disabled.
7. Prove delivery/replay/busy/cancellation/restart-loss behavior.
8. Separately consider continuation through existing direct-turn admission.
9. Add TmuxWindow and Subagent adapters only if each remains thin.

At every step, identify which authority disappears. Never run old and new writers together.

## Smallest proposed end-to-end slice

Bash durable delivery only; no automatic continuation:

1. Start a background Bash handle.
2. Explicitly call `wait_until` for that handle.
3. Return the conversation to Idle.
4. Bash exits.
5. Bash lifecycle authority records one typed generic workflow observation.
6. Generic workflow creates one durable delivery.
7. Existing direct-turn materialization writes one correlated conversation message.
8. If the conversation is busy, no competing turn starts.

Required proof:

- Ordinary Bash never enrolls.
- Unauthorized/missing handles reject registration.
- One exit creates one result; replay creates no duplicate.
- Crash between observation and message materialization recovers exactly once.
- Busy conversation retains the result without continuation.
- Wait cancellation does not kill Bash or discard the eventual result.
- Wait cancellation suppresses automatic continuation.
- Phoenix restart produces typed Bash restart loss.

## Stop gates

Stop and redesign if any slice requires:

- a wake-specific worker;
- a second runtime-admission path;
- duplicated resource lifecycle state;
- dual writes or a translation bridge;
- a generic provider/subscription framework;
- delivery-owner transfer;
- polling;
- broad UI lifecycle state.

## Open review questions

- Can direct-turn materialization accept a non-user-originated typed result without weakening direct-turn identity or user-message invariants?
- Can generic workflow delivery target existing direct-turn admission directly, or is one narrow profile-specific reducer event required?
- What is the exact durable correlation key: registering tool-use ID, workflow ID, or a typed pair?
- Does cancellation need a persisted policy bit, generic delivery suppression, or both?
- Can Bash restart loss be produced deterministically at startup without a wake-specific scanner?
- Can tmux completion be event-driven with one startup reprobe rather than recurring polling?
- Can Subagent terminal observation attach to the existing `PersistSubAgentResults` transaction without duplicating terminal truth?
- Which legacy tables and API surfaces contain historical data that should be dropped versus retained only for audit?

## Evidence anchors

- Generic workflow durability: `WorkflowRepository::begin_attempt`, `record_observation`, `WorkflowTx::resolve_deliveries_exact`.
- Direct-turn durability: `claim_authoritative_turn`, `materialize_authoritative_user_message`.
- Busy authority: `RuntimeManager::conversation_admission`.
- Bash truth: `BashHandleRegistry`, `Handle::transition_to_terminal`.
- Tmux truth: `TmuxRegistry`, deterministic socket/server token, startup reprobe.
- Subagent truth: state-machine terminal transitions and `PersistSubAgentResults`.

## Non-goals

No implementation, schema migration, production cancellation, deployment, UI, combinators, generic providers, or ownership transfer is authorized by this task snapshot.
