# Wake authority and pure lifecycle model

## Status

PR #559 is closed as superseded. Its branch and all commits remain available as evidence and cherry-pick sources. This task defines the model before production integration.

This task is a bounded design foundation, not a usable wake feature. It defines and tests the semantic authority and transactional seams needed to judge whether production integration is justified. It does not register a Bash wait, observe a real handle, expose wake status, or resume a conversation.

## Concrete user problem

An agent that starts long-running work must currently spend turns and tokens polling for completion. The desired experience is one explicit request—“deliver the terminal result of this handle when it becomes known”—after which the conversation remains available for user input and eventually receives one correlated terminal tool result without polling.

The harmful failures this foundation prevents are narrower and concrete:

- silently losing the wait or its terminal result across a Phoenix crash;
- delivering a crossed or stale handle observation to the wrong contract;
- producing two canonical terminal outcomes when observation races cancellation or expiry;
- resuming the wrong conversation after continuation;
- persisting terminal truth while omitting or corrupting its delivery obligation.

This task does **not** prove that users need a generic multi-handle wake platform. It proves a candidate authority model against those failure classes so downstream work can be accepted or rejected with evidence.

## Panel synthesis

Five perspectives—durable workflow/state machine, SQLite recovery, concurrency/cancellation, SSE/projection, and product/runtime semantics—converged on one boundary:

- The wake-contract aggregate is the sole semantic authority. It owns a closed lifecycle and one terminalization per contract generation.
- Adapters provide typed observations only. They cannot decide product terminal meaning, allocate public contract identity, or mutate conversation state.
- A pure total transition function computes `State + Command -> Outcome + NewState + OwedEffects`.
- The repository commits the transition products atomically: aggregate head/state, canonical terminal evidence, receipt, exact delivery, lifecycle event, sequence-barrier membership, and attempt/lease revocation.
- Claims, leases, retries, and dispatch attempts are execution facts—not aggregate lifecycle states.
- Contract identity is added once at the canonical contract-event boundary. Evidence remains substrate-typed and identity-free before that boundary.
- Public API, transcript, live SSE, replay, generated TypeScript, and UI status are projections. They are rebuildable and never become parallel authorities.
- Explicit `wait_until` registers a contract. A successful whole tool round then settles normally from `ToolExecuting` to `Idle`; the aggregate does not add a conversation "waiting" state.
- Exactly-once means one durable terminal materialization and at-most-once automatic resumption. SSE is advisory/replayable projection traffic.

## Foundation boundary

The foundation owns four things only:

1. a pure, total lifecycle model with one canonical terminal outcome;
2. sealed authorization boundaries for registration, observation, and delivery-owner transfer;
3. a wake-specific repository transaction that is the only semantic mutation path for wake workflows;
4. tests and traceability showing how historical race, recovery, and authority findings constrain the model.

The generic workflow engine supplies storage primitives, IDs, attempts, receipts, and exact deliveries, but it is not a parallel semantic authority. Public generic mutation wrappers reject wake profiles; wake repository operations derive semantic fields from persisted wake state rather than caller-provided snapshots.

Explicitly outside this task:

- production capability minting or observation in Bash, tmux, or sub-agent adapters;
- `wait_until` tool behavior or descriptions;
- router loops, timers, polling cadence, or process control;
- API, SSE, transcript, CLI, or UI projections;
- automatic conversation resumption;
- migration or compatibility of the superseded PR #559 implementation;
- evidence that the broader multi-substrate product scope is worth building.

A test-support constructor demonstrates a model transition; it is not production authorization. Downstream adapters must define where real capabilities are minted and prove that their resource ownership cannot be forged.

## Resolved product policy

- **Occurrence wins:** terminal evidence whose real occurrence time precedes authoritative cancellation remains canonical even if persisted later. Otherwise cancellation wins.
- **Cancellation is observation-only:** cancelling a wake closes observation/resumption. It does not signal or kill the underlying Bash resource; process control is a separate explicit command.
- **Delivery while busy:** terminal delivery is durable and queued; automatic resumption waits until the current authoritative turn settles.
- **Ownership transfer:** continuation transfers delivery ownership, not workflow or resource identity.
- **Deadline occurrence precedence:** evidence occurring at or before the deadline remains eligible even when observed after expiry is proposed; evidence after the deadline cannot replace expiry.
- **Bounded registration:** wake deadlines are strictly after registration and no more than 1,800 seconds later; adapters reject zero or over-bound durations before registration.
- **Identity split:** registration ownership is immutable audit identity; continuation changes only the current delivery owner while preserving contract, workflow, and resource identity.
- **Registration authority:** registration and delivery owners are nominal non-empty identities. Registration also durably binds the originating tool-use identity in aggregate, canonical events, and normalized relational state so terminal delivery can address the original invocation after restart.
- **Authorized subjects:** registration accepts an authorization-bound subject capability supplied by the resource-owning adapter, not a freely constructed resource description. This model PR seals the boundary; the Bash integration supplies its production minting path.
- **Transferability policy:** WorkScope-keyed resources permit delivery-owner transfer; fixed-owner resources, including sub-agent-like resources, reject transfer.
- **Fence scope:** a durable fence receipt gates only finalization of the proposed cancellation or expiry. Earlier authoritative fired or forgotten evidence may supersede the proposal without that receipt.
- **Execution authority:** terminal evidence carries a sealed observation capability bound to contract identity, generation, resource identity, and effect attempt; crossed or stale workers cannot close another contract.
- **Transfer authority:** delivery-owner changes carry a sealed continuation/WorkScope capability binding both current and successor owners; transferability alone does not authorize redirection.
- **Acceptance lifecycle:** fired, expired, and forgotten contracts remain workflow-active while runtime acceptance is owed. Generic transition and delivery-resolution wrappers reject wake profiles so only the wake-specific acceptance path may settle that debt.
- **Rebuildable policy:** registration events include the typed delivery-transferability policy; replay never guesses policy from profile naming.
- **Profile identity:** wake profile kind and version are validated nominal types, structurally matching the normalized relational constraints.
- **Codec identity:** resource and terminal-evidence codec families are non-empty and versions are positive at the model boundary, matching normalized SQL constraints.
- **Forgotten reasons:** forgotten terminals use only the normative finite reasons: Phoenix restart, cascade-destroyed handle, missing sub-agent handle, or missing tmux handle.
- **Wake-specific settlement:** the wake repository owns terminal runtime acceptance and fence receipt acknowledgement. Terminal acceptance atomically settles the exact owed delivery; fence acknowledgement persists the receipt without exposing an internal reducer delivery.
- **Transition identity:** transition zero is rejected by the pure model before persistence.
- **Repository-derived acceptance:** wake terminal acceptance accepts only the repository-discovered owed delivery and derives generation, status, codecs, event, and snapshot from the loaded closed aggregate.
- **Cancellation settlement:** non-resuming cancellation deliveries are accepted atomically with terminalization and never remain pending.
- **Transactional identity allocation:** wake registration allocates its workflow identity from the shared global sequence inside the same transaction that inserts the workflow and authority binding.
- **Receipt rollback:** rejected fence acknowledgement rolls back all attempted effect and receipt mutations.

## Cancellation and expiry policy

Cancellation is a request to stop the wait, not a command to stop the underlying resource. Killing a Bash process, closing tmux, or cancelling a sub-agent remains a separate explicit operation.

Cancellation and expiry use the same bounded arbitration protocol:

1. the repository records a proposed terminal cutoff and fences this contract generation's observation attempt;
2. terminal evidence or authoritative resource-loss evidence whose occurrence time is at or before the cutoff may supersede the proposal;
3. evidence occurring after the cutoff cannot supersede it;
4. after the fence receipt proves the attempt cannot produce undiscovered earlier evidence, the repository finalizes the proposed cancellation or expiry;
5. finalization and its synthetic delivery settlement commit atomically;
6. cancellation's delivery is settled without scheduling an LLM turn, while fired, expired, and forgotten deliveries remain owed until the runtime accepts them.

The fence is local to cancellation/expiry arbitration. It is not required for an already-authoritative earlier `Fired` or `Forgotten` outcome, and it is not permission to kill the watched resource. If downstream integration cannot mint, receipt, and recover this fence without a second semantic authority, the adapter is not admissible under this foundation.

## Authoritative aggregate

```rust
struct WakeContract {
    id: WakeContractId,
    generation: Generation,
    registration_owner: WakeOwner,
    delivery_owner: WakeOwner,
    registering_tool_use_id: RegisteringToolUseId,
    subject: WakeSubject,
    delivery_transferability: WakeDeliveryTransferability,
    condition: WakeCondition,
    registered_at: OccurredAt,
    deadline: OccurredAt,
    state: WakeLifecycle,
}

enum WakeLifecycle {
    Open,
    Closed(CanonicalTerminal),
}

enum CanonicalTerminal {
    Fired { evidence: TerminalEvidence, occurred_at: OccurredAt },
    Expired { deadline: OccurredAt },
    Cancelled { cause: CancellationCause, occurred_at: OccurredAt },
    Forgotten { cause: ForgottenCause, occurred_at: OccurredAt },
}
```

Cancellation and forgotten causes are closed enums. Adapter protocol failures reconcile through typed forgotten causes rather than introducing a fifth terminal family. Diagnostics are adjunct typed data, never authority-bearing strings.

`TerminalProposed` is an open semantic arbitration substate, not terminal truth or an execution retry state. Entering it fences this contract generation's observation authority. Finalization requires a nominal observation-fence proof bound to the contract, generation, and proposal transition; this is profile-local reconciliation of already-authoritative evidence, not the permanent engine-wide exact-drain machinery retired by ADR-019.

Registration ownership is immutable and structurally non-empty. A separate non-empty delivery owner names the conversation that receives canonical delivery and may change during continuation only when the registered subject is WorkScope-transferable, without rewriting registration attribution or resource identity. Fixed-owner subjects reject transfer.
Registration durably carries the non-empty registering tool-use identity through the aggregate, canonical events, and normalized authority row. Registration accepts an authorization-bound subject capability minted by the resource-owning adapter; the unrestricted subject value is retained only after that capability is validated and consumed. The Bash adapter integration owns the production capability-minting API.
The subject declares the exact terminal-evidence codec accepted by the profile; evidence with any other codec family/version is rejected before occurrence arbitration.

The durable authority row normalizes contract identity, owners, profile/resource/evidence codecs, deadline, lifecycle, and terminal occurrence time for SQL validation and recovery. Polymorphic payloads and event bodies remain earned blobs; normalized facts are not inferred from those blobs.

## Pure transition contract

Commands are closed: `Register`, `Observe`, `Cancel`, `DeadlineElapsed`, `TransferDeliveryOwner`, and `Reconcile`. Registration, observation, and ownership transfer consume sealed capabilities minted by their owning adapter or WorkScope authority; public commands cannot manufacture resource authority from IDs. Every `(state, command)` pair returns a typed accepted/replayed/rejected outcome, a new state, and owed effects. No semantic branch panics or relies on wall-clock polling.

Owed effects include adapter watch/unwatch, reducer inbox delivery, public projection append, and lifecycle cleanup. An effect is idempotently keyed by contract generation and transition identity. Effect completion never rewrites aggregate truth.

Replay requires the exact semantic command and transition identity. Reusing a transition identity with a different command kind, head, or payload is a typed conflict. Every accepted command advances the composite `(generation, version)` head exactly once; stale same-generation versions cannot mutate authority.
Transition identities are monotonic within a contract generation. Exact replay is valid only at the current head; any older identity is rejected even when its payload matches a historical command.
Proposal capabilities are bound to immutable contract identity, generation, and the proposal transition rather than the mutable head version. Delivery-owner transfer therefore preserves the capability; restart recovery reconstructs finalization only after the repository verifies the proposal's durable fence receipt. A fence receipt is required only when finalizing that proposed cancellation or expiry; occurrence-precedence evidence that terminalized earlier may close the contract as fired or forgotten without waiting for the proposal fence.

Repository `committed_at` records transaction commit time. Domain occurrence timestamps remain exclusively in terminal evidence/proposals and never substitute for storage audit time.

The model boundary exposes an exhaustive public-event registry. Registration events contain the full immutable registration facts, transfer events contain both delivery owners, and terminal events contain canonical terminal evidence plus the delivery owner, so downstream projections can be rebuilt without becoming authorities.

## Repository transaction

One coarse `commit_transition(plan)` API validates expected generation/version and atomically writes all semantic consequences. Low-level head-CAS/receipt/delivery/barrier primitives are not exposed to profile code. Terminal commit guarantees either all or none of:

1. closed aggregate head and append-only transition;
2. one canonical terminal evidence record with real occurrence time;
3. one receipt and one exact delivery keyed to that receipt;
4. one lifecycle public-event record and required sequence barrier;
5. revocation of live attempts and reclaimable leases;
6. owed acceptance/projection inputs.

Projection rows are derived/rebuildable. Stale observations may be retained for audit but cannot create a second receipt.

## Replacement PR ancestry

```text
origin/main
  └─ PR 1 / task 44011: wake authority + pure model
       └─ PR 2 / task 44012: explicit Bash wait_until adapter
            └─ PR 3 / task 44013: public projections/event registry
                 └─ PR 4 / task 44014: later TmuxWindow adapter
```

Each branch starts from the merged predecessor. PR #559 is never merged; useful commits/tests are cherry-picked selectively.

## Historical finding matrix

Every inline P1/P2 is an obligation even when its thread was resolved or outdated. Classification determines its required test form; the original comment id and reviewed commit remain exact traceability.

### Foundation-review closure

Codex reviewed the evolving foundation in six rounds and reported twenty-six actionable threads. The comments were not independent edge cases; they exposed five missing boundary statements now made explicit above.

| Boundary exposed | Review threads | Foundation decision | Evidence surface |
|---|---|---|---|
| Terminal truth is distinct from runtime admission and delivery settlement | `PRRT_kwDORKxuOM6XHG60`, `PRRT_kwDORKxuOM6XHw-H`, `PRRT_kwDORKxuOM6XINE-`, `PRRT_kwDORKxuOM6XINFB`, `PRRT_kwDORKxuOM6XIsGn` | The foundation creates an exact reducer delivery and leaves it `Owed`; production runtime admission/acceptance is deliberately deferred rather than simulated in the repository. Cancellation settles without resume. | Repository terminalization, cancellation, and restart reload tests; no foundation acceptance API |
| Generic workflow storage is not wake semantic authority | `PRRT_kwDORKxuOM6XHG64`, `PRRT_kwDORKxuOM6XIsGt`, `PRRT_kwDORKxuOM6XJAzg` | Generic creation, transition, delivery, and receipt wrappers reject wake profiles; wake repository operations derive writes from loaded aggregate state. | Generic creation/wrapper/receipt rejection and projection-consistency tests |
| IDs and transferability are not authorization | `PRRT_kwDORKxuOM6XHG6-`, `PRRT_kwDORKxuOM6XHG7A`, `PRRT_kwDORKxuOM6XHw-O`, `PRRT_kwDORKxuOM6XINFD`, `PRRT_kwDORKxuOM6XJAzT`, `PRRT_kwDORKxuOM6XJAzm` | Registration, observation, reconciliation, transfer, and workflow allocation require resource/repository authority rather than caller-selected identifiers; registration retries recover the existing contract binding and effect IDs are range-bounded. | Crossed/stale capability, exact registration replay, shared-sequence, and transition/effect-ID bound tests |
| Persisted events, effects, and types must be total | `PRRT_kwDORKxuOM6XHG67`, `PRRT_kwDORKxuOM6XHG6_`, `PRRT_kwDORKxuOM6XHw-Q`, `PRRT_kwDORKxuOM6XHw-U`, `PRRT_kwDORKxuOM6XHw-X`, `PRRT_kwDORKxuOM6XIsGo`, `PRRT_kwDORKxuOM6XJAza` | Registration events retain transfer policy; effect intents round-trip for restart decoding; closed enums and duration bounds match SQL; nominal identities reject SQL-invalid values before transition. | Event/effect rebuildability, serde rejection, relational-bound, and migration tests |
| Cancellation/expiry fencing is an internal transaction protocol | `PRRT_kwDORKxuOM6XHw-L`, `PRRT_kwDORKxuOM6XINFA`, `PRRT_kwDORKxuOM6XJAzt` | Fence acknowledgement creates no reducer delivery and rejected acknowledgement rolls back every mutation; the receipt gates only proposal finalization; evidence must precede both proposal cutoff and contract deadline. | Fence lifecycle, rollback, no-delivery, deadline, and occurrence-precedence tests |
| Generic delivery schema and wake ownership are separate dimensions | `PRRT_kwDORKxuOM6XIsGu` | Generic `consumer_kind` remains the supported `reducer` discriminator; mutable wake delivery ownership stays in the normalized wake authority row and terminal event. | Terminal bundle insertion and delivery-owner projection tests |
| The foundation cannot claim a production lifecycle | `PRRT_kwDORKxuOM6XIsGp` | Registration is intentionally unreachable outside test support until an adapter owns capability minting; the task and PR describe this as a bounded model, not a working wake feature. | Production API surface review and downstream decision gate |

This closure table is the review-facing index. The larger matrix below preserves the complete historical evidence from superseded PR #559 so downstream work can select regressions without reviving its distributed authority model.

### Aggregate invariant (11)

| Severity | Finding | Comment | Surface | Reviewed commit |
|---|---|---:|---|---|
| P2 | Scope wake recovery suppression to the parked turn | `3622435661` | `crates/phoenix-ide/src/runtime.rs` | `d4d3b2d7a906` |
| P1 | Update the stale work-scope expectation | `3625568531` | `crates/phoenix-tools/src/wait_until.rs` | `a041c64bd548` |
| P2 | Preserve wake bindings across Explore approval scope flips | `3627100353` | `crates/phoenix-tools/src/lib.rs` | `825978c5ccf5` |
| P2 | Preserve Bash command identity in wake results | `3634202279` | `crates/phoenix-ide/src/runtime/wake.rs` | `f0af61705d17` |
| P2 | Rekey wake bindings only after resource moves succeed | `3646389353` | `crates/phoenix-ide/src/runtime/executor.rs` | `4f4284f145f8` |
| P1 | Stop implicitly registering wakes for tmux runs | `3651806609` | `crates/phoenix-tools/src/tmux/run.rs` | `dfd6cf356733` |
| P2 | Preserve earlier registrations in the same tool round | `3651806617` | `crates/phoenix-db/src/workflow/wake.rs` | `dfd6cf356733` |
| P1 | Enforce handle access before registering waits | `3651888619` | `crates/phoenix-tools/src/wait_until.rs` | `2cdc9fb2f6b9` |
| P2 | Publish registration before waking the terminal worker | `3653034062` | `crates/phoenix-ide/src/runtime/executor.rs` | `7961e80bbb21` |
| P1 | Stop advertising tmux wakes that cannot activate | `3653261927` | `crates/phoenix-db/src/lib.rs` | `092fd748ae1d` |
| P2 | Include contract identity in waiter-panic observations | `3680169063` | `crates/phoenix-ide/src/runtime/wake.rs` | `8ba812ca6d9d` |

### Transition/property (49)

| Severity | Finding | Comment | Surface | Reviewed commit |
|---|---|---:|---|---|
| P1 | Suppress recovery auto-continue for parked wake waits | `3619380953` | `crates/phoenix-state-machine/src/transition.rs` | `9c4f1eeb6a7c` |
| P2 | Preserve old in-flight tool states during upgrade | `3619380956` | `crates/phoenix-core/src/domain/sm_state.rs` | `9c4f1eeb6a7c` |
| P2 | Deliver wake completions as tool results | `3619380961` | `crates/phoenix-tools/src/wait_until.rs` | `9c4f1eeb6a7c` |
| P2 | Carry park intent through sub-agent fan-in | `3619380964` | `crates/phoenix-state-machine/src/transition.rs` | `9c4f1eeb6a7c` |
| P2 | Skip deleted generated files when normalizing codegen output | `3622752674` | `dev.py` | `34cad1697c18` |
| P2 | Map duplicate handle waits to a typed conflict | `3624894089` | `crates/phoenix-tools/src/wait_until.rs` | `01342ed0c164` |
| P2 | Add wait_until to sandboxed Explore | `3625568518` | `crates/phoenix-tools/src/lib.rs` | `a041c64bd548` |
| P2 | Do not park after sibling tool errors | `3625568527` | `crates/phoenix-state-machine/src/transition.rs` | `a041c64bd548` |
| P2 | Add wait_until to Work sub-agents | `3625817162` | `crates/phoenix-tools/src/lib.rs` | `08616d782797` |
| P2 | Avoid accepting wake adoption on a stale runtime | `3626798421` | `crates/phoenix-ide/src/runtime/wake.rs` | `e170669b6bdc` |
| P2 | Resume immediately after failed sub-agent fan-in | `3626798425` | `crates/phoenix-state-machine/src/transition.rs` | `e170669b6bdc` |
| P2 | Avoid accepting wake adoption on a stale runtime | `3626798426` | `crates/phoenix-ide/src/runtime/wake.rs` | `e170669b6bdc` |
| P2 | Keep sub-agent streams open while parked on wakes | `3627100365` | `crates/phoenix-tools/src/lib.rs` | `825978c5ccf5` |
| P2 | Use the adopted batch's auto-resume decision | `3627423389` | `crates/phoenix-ide/src/runtime/wake.rs` | `a0024f066c57` |
| P2 | Preserve bash wake tail window metadata | `3632376627` | `crates/phoenix-ide/src/runtime/wake.rs` | `05721e34742e` |
| P2 | Recheck new runtimes before adopting wakes | `3632376634` | `crates/phoenix-ide/src/runtime/wake.rs` | `05721e34742e` |
| P2 | Do not suppress adopted wake-result recovery on siblings | `3634202269` | `crates/phoenix-ide/src/runtime/recovery.rs` | `f0af61705d17` |
| P2 | Kick the worker after wake activation | `3638186255` | `crates/phoenix-db/src/workflow/wake.rs` | `2c6395be4f9d` |
| P2 | Batch ready wake deliveries before adopting | `3638186261` | `crates/phoenix-ide/src/runtime/wake.rs` | `2c6395be4f9d` |
| P2 | Honor advertised non-Bash wait handles | `3645061764` | `crates/phoenix-tools/src/wait_until.rs` | `05ecf307f4b1` |
| P2 | Order batched wake deliveries deterministically | `3645061771` | `crates/phoenix-ide/src/runtime/wake.rs` | `05ecf307f4b1` |
| P2 | Resume immediately when a parked batch has errors | `3646389351` | `crates/phoenix-state-machine/src/transition.rs` | `4f4284f145f8` |
| P2 | Keep the terminal payload out of display data | `3647064239` | `crates/phoenix-ide/src/runtime/wake.rs` | `ebf9c27f972d` |
| P2 | Don't suppress interrupted sibling recovery behind wakes | `3647064246` | `crates/phoenix-ide/src/runtime/recovery.rs` | `ebf9c27f972d` |
| P2 | Bound each per-conversation wake batch | `3651888618` | `crates/phoenix-ide/src/runtime/wake.rs` | `2cdc9fb2f6b9` |
| P2 | Retire stale bindings created in the same second | `3651888621` | `crates/phoenix-db/src/workflow/wake.rs` | `2cdc9fb2f6b9` |
| P2 | Do not park after earlier tool failures | `3652604141` | `crates/phoenix-state-machine/src/transition.rs` | `429efb3e422c` |
| P2 | Sort pending deliveries before truncating the batch | `3653034060` | `crates/phoenix-ide/src/runtime/wake.rs` | `7961e80bbb21` |
| P2 | Use the Bash wait timestamp encoding for wake results | `3653198294` | `crates/phoenix-ide/src/runtime/wake.rs` | `54497f7d02da` |
| P1 | Serialize direct-turn acceptance with wake adoption | `3653261930` | `crates/phoenix-ide/src/runtime/wake.rs` | `092fd748ae1d` |
| P2 | Strip normalized tails from workflow snapshots and events | `3653592991` | `crates/phoenix-db/src/workflow/wake.rs` | `59147ff05a95` |
| P1 | Wait for tmux completion before closing the window | `3653662167` | `crates/phoenix-tools/src/tmux/run.rs` | `ed26fefae85f` |
| P2 | Preserve commit order for same-second wake resolutions | `3653828422` | `crates/phoenix-ide/src/runtime/wake.rs` | `b5ff43f7a568` |
| P1 | Order committed wakes before later direct turns | `3654197913` | `crates/phoenix-ide/src/runtime/wake.rs` | `389755cde18d` |
| P1 | Adopt only the bounded materialized wake batch | `3654197919` | `crates/phoenix-ide/src/runtime/wake.rs` | `389755cde18d` |
| P1 | Strip all normalized Bash fields from workflow blobs | `3662614704` | `crates/phoenix-db/src/workflow/wake.rs` | `46a4fde1e7f2` |
| P2 | Keep rounded evidence from entering the future | `3676951302` | `crates/phoenix-ide/src/runtime/wake.rs` | `49c5d8e5e855` |
| P2 | Let undeliverable wakes be cleared without a runtime | `3676951311` | `crates/phoenix-ide/src/runtime/wake.rs` | `49c5d8e5e855` |
| P2 | Restore completion cleanup for readiness windows | `3679370079` | `crates/phoenix-tools/src/tmux/run.rs` | `36350a9a407e` |
| P2 | Fence cleanup to the original tmux server | `3679446470` | `crates/phoenix-tools/src/tmux/run.rs` | `f49488938b9f` |
| P2 | Truncate wake tails before JSON serialization | `3679611212` | `crates/phoenix-ide/src/runtime/wake.rs` | `994787ee0389` |
| P2 | Preserve the end of oversized wake tails | `3679705906` | `crates/phoenix-ide/src/runtime/wake.rs` | `006fcf9b0309` |
| P2 | Drain committed wake debt before accepting a direct turn | `3679873556` | `crates/phoenix-ide/src/send_chat_service.rs` | `2566e16519cb` |
| P2 | Close inline streams on sub-agent terminal events | `3679873560` | `ui/src/hooks/useConversationInlineStream.ts` | `2566e16519cb` |
| P2 | Verify tmux cleanup before retiring the cleanup task | `3680007393` | `crates/phoenix-tools/src/tmux/run.rs` | `8d48b05a339f` |
| P2 | Record waiter-panic time instead of epoch zero | `3680169060` | `crates/phoenix-ide/src/runtime/wake.rs` | `8ba812ca6d9d` |
| P2 | Align the parking transition with the unchanged-state rule | `3680169072` | `crates/phoenix-state-machine/src/transition.rs` | `8ba812ca6d9d` |
| P2 | Preserve already-terminal evidence for immediate waits | `3683098272` | `crates/phoenix-ide/src/runtime/wake.rs` | `35e734b0c971` |
| P2 | Recheck the wake before parking after sub-agent fan-in | `3683098281` | `crates/phoenix-state-machine/src/transition.rs` | `35e734b0c971` |

### Crash/repository atomicity (18)

| Severity | Finding | Comment | Surface | Reviewed commit |
|---|---|---:|---|---|
| P2 | Poll live wake handles before the lease deadline | `3622435657` | `crates/phoenix-ide/src/runtime/wake.rs` | `d4d3b2d7a906` |
| P2 | Match wake suppression to the registration receipt | `3622752650` | `crates/phoenix-db/src/workflow/wake.rs` | `34cad1697c18` |
| P2 | Release overdue wake leases before expiry | `3625568521` | `crates/phoenix-ide/src/runtime/wake.rs` | `a041c64bd548` |
| P2 | Emit wake registration after checkpoint succeeds | `3625817167` | `crates/phoenix-ide/src/runtime/executor.rs` | `08616d782797` |
| P2 | Identify wait receipts structurally during recovery | `3630022509` | `crates/phoenix-ide/src/runtime/recovery.rs` | `53d7d95eed8d` |
| P2 | Commit wake registration with the tool checkpoint | `3632376617` | `crates/phoenix-tools/src/wait_until.rs` | `05721e34742e` |
| P2 | Preserve the real tool-round ID in registration receipts | `3651888614` | `crates/phoenix-db/src/workflow/wake.rs` | `2cdc9fb2f6b9` |
| P2 | Release observation authority after persistence errors | `3653741143` | `crates/phoenix-ide/src/runtime/wake.rs` | `59dc1d2803eb` |
| P1 | Map wake receipt columns explicitly during migration | `3653828419` | `crates/phoenix-db/src/migrations.rs` | `b5ff43f7a568` |
| P2 | Admit waiter panic in the normative forgotten-reason set | `3654197914` | `crates/phoenix-db/src/migrations.rs` | `389755cde18d` |
| P1 | Preserve the wake across checkpoint persistence failures | `3678552996` | `crates/phoenix-ide/src/runtime/executor.rs` | `43187c57884f` |
| P2 | Release the lease when forgotten persistence fails | `3679258251` | `crates/phoenix-ide/src/runtime/wake.rs` | `2665aaca9e50` |
| P2 | Fence wake activation until checkpoint publication finishes | `3679258258` | `crates/phoenix-db/src/lib.rs` | `2665aaca9e50` |
| P1 | Retry the consumed outcome after checkpoint rollback | `3679505971` | `crates/phoenix-ide/src/runtime/executor.rs` | `7b51ae17d882` |
| P2 | Run tmux completion cleanup before fallible delivery | `3679611198` | `crates/phoenix-ide/src/runtime/wake.rs` | `994787ee0389` |
| P2 | Preserve unknown legacy tail offsets | `3679779596` | `crates/phoenix-db/src/migrations.rs` | `ebb412bf8a7f` |
| P2 | Retry only the failed persistence boundary | `3679931307` | `crates/phoenix-ide/src/runtime/executor.rs` | `a1b4203bfcdb` |
| P2 | Retain Bash waiter-panic diagnostics in wake delivery | `3680007390` | `crates/phoenix-ide/src/runtime/wake.rs` | `8d48b05a339f` |

### Interleaving/cancellation (33)

| Severity | Finding | Comment | Surface | Reviewed commit |
|---|---|---:|---|---|
| P2 | Handle parked tool completions while cancelling | `3619380965` | `crates/phoenix-state-machine/src/transition.rs` | `9c4f1eeb6a7c` |
| P2 | Derive expiry from the registration timestamp | `3622752664` | `crates/phoenix-tools/src/wait_until.rs` | `34cad1697c18` |
| P2 | Cancel wake registrations from earlier siblings | `3622752693` | `crates/phoenix-state-machine/src/transition.rs` | `34cad1697c18` |
| P2 | Handle kill-pending Bash wakes as terminal | `3624542542` | `crates/phoenix-ide/src/runtime/wake.rs` | `e1234803b6c6` |
| P2 | Rekey direct-scope waits on continuation | `3624894096` | `crates/phoenix-tools/src/wait_until.rs` | `01342ed0c164` |
| P2 | Stop carrying parked wake ids into later cancels | `3626798424` | `crates/phoenix-ide/src/runtime/executor.rs` | `e170669b6bdc` |
| P2 | Stop carrying parked wake ids into later cancels | `3626798428` | `crates/phoenix-ide/src/runtime/executor.rs` | `e170669b6bdc` |
| P2 | Cancel parked sub-agent wakes when cancelling the child | `3627100357` | `crates/phoenix-tools/src/lib.rs` | `825978c5ccf5` |
| P2 | Do not let inactive wake bindings block retries | `3634202263` | `crates/phoenix-db/src/workflow/wake.rs` | `f0af61705d17` |
| P2 | Use stable kill-signal strings in Bash wake evidence | `3634202282` | `crates/phoenix-ide/src/runtime/wake.rs` | `f0af61705d17` |
| P2 | Preserve kill-pending wait timestamp shape | `3635422246` | `crates/phoenix-ide/src/runtime/wake.rs` | `1e2717873f9e` |
| P2 | Preserve live partial output in kill-pending wakes | `3638186272` | `crates/phoenix-ide/src/runtime/wake.rs` | `2c6395be4f9d` |
| P2 | Preserve wake ids when cancellation fails | `3645610552` | `crates/phoenix-ide/src/runtime/executor.rs` | `4c041b1e1db3` |
| P2 | Do not drop the only tool outcome on wake-cancel errors | `3645610562` | `crates/phoenix-ide/src/runtime/executor.rs` | `4c041b1e1db3` |
| P1 | Process user cancellation despite wake cleanup failures | `3651806614` | `crates/phoenix-ide/src/runtime/executor.rs` | `dfd6cf356733` |
| P2 | Fence wake registration against lifecycle admission | `3652604145` | `crates/phoenix-tools/src/wait_until.rs` | `429efb3e422c` |
| P2 | Preserve the kill-attempt timestamp for killed wakes | `3653198292` | `crates/phoenix-ide/src/runtime/wake.rs` | `54497f7d02da` |
| P1 | Recheck direct-turn admission under the shared lock | `3653322374` | `crates/phoenix-ide/src/runtime/wake.rs` | `d779d17dcc86` |
| P2 | Close lifecycle admission before registration | `3653548512` | `crates/phoenix-db/src/workflow/wake.rs` | `9e9265c6871b` |
| P2 | Resolve waiter-panicked Bash handles instead of polling them | `3653592993` | `crates/phoenix-ide/src/runtime/wake.rs` | `59147ff05a95` |
| P2 | Retry wake-cancellation debt automatically | `3653741140` | `crates/phoenix-ide/src/runtime/executor.rs` | `59dc1d2803eb` |
| P2 | Scope lifecycle admission locks per conversation | `3653741147` | `crates/phoenix-ide/src/api/handlers.rs` | `59dc1d2803eb` |
| P2 | Reclaim per-conversation admission locks | `3653828424` | `crates/phoenix-ide/src/runtime.rs` | `b5ff43f7a568` |
| P2 | Scope the wake worker's admission lock per conversation | `3657570823` | `crates/phoenix-ide/src/runtime/wake.rs` | `560de50251c8` |
| P2 | Preserve subsecond evidence time at the deadline | `3662289446` | `crates/phoenix-ide/src/runtime/wake.rs` | `eb6e9f80c064` |
| P2 | Serialize wake cancellation before direct-turn admission | `3677034777` | `crates/phoenix-ide/src/send_chat_service.rs` | `5af9cd065cbc` |
| P2 | Re-key wakes during live continuation handoff | `3679446474` | `crates/phoenix-ide/src/runtime/wake.rs` | `f49488938b9f` |
| P2 | Keep kill-pending Bash wakes unresolved | `3679611194` | `crates/phoenix-ide/src/runtime/wake.rs` | `994787ee0389` |
| P2 | Reconcile zero-second waits with the expiry invariant | `3679611204` | `crates/phoenix-tools/src/wait_until.rs` | `994787ee0389` |
| P2 | Cancel registrations hidden by tool cancellation | `3679611209` | `crates/phoenix-tools/src/wait_until.rs` | `994787ee0389` |
| P2 | Ignore superseded wake-status responses | `3679705905` | `ui/src/components/WakeStatusBar.tsx` | `006fcf9b0309` |
| P2 | Preserve wakes during internal sub-agent timeout | `3680169068` | `crates/phoenix-ide/src/runtime/executor.rs` | `8ba812ca6d9d` |
| P2 | Retain cancellation debt when abort cleanup fails | `3683098266` | `crates/phoenix-ide/src/runtime/executor.rs` | `35e734b0c971` |

### Projection/exhaustiveness (28)

| Severity | Finding | Comment | Surface | Reviewed commit |
|---|---|---:|---|---|
| P2 | Handle wake registration in pending replay | `3622123198` | `crates/phoenix-ide/src/runtime/executor.rs` | `b6bd438e6fdf` |
| P2 | Make wait_until registration replay-stable | `3622123204` | `crates/phoenix-tools/src/wait_until.rs` | `b6bd438e6fdf` |
| P2 | Do not replay resolved wait registrations | `3622752682` | `crates/phoenix-tools/src/wait_until.rs` | `34cad1697c18` |
| P2 | Do not burn SSE sequence ids on wake retry | `3624894079` | `crates/phoenix-ide/src/api.rs` | `01342ed0c164` |
| P2 | Render forgotten Bash wakes with the wait-result shape | `3638186268` | `crates/phoenix-ide/src/runtime/wake.rs` | `2c6395be4f9d` |
| P2 | Render cancelled Bash wakes with a stable status | `3642935210` | `crates/phoenix-ide/src/runtime/wake.rs` | `c1bb547481c0` |
| P2 | Emit terminal wake SSE events | `3645061780` | `crates/phoenix-ide/src/runtime/wake.rs` | `05ecf307f4b1` |
| P2 | Consume terminal wake events during init replay | `3645610549` | `ui/src/conversation/atom.ts` | `4c041b1e1db3` |
| P2 | Avoid leaving holes in wake SSE reservations | `3645610558` | `crates/phoenix-ide/src/runtime/wake.rs` | `4c041b1e1db3` |
| P2 | Include the condition in wake-registration SSE events | `3651888615` | `crates/phoenix-ide/src/api/wire.rs` | `2cdc9fb2f6b9` |
| P1 | Release the checkpoint sequence range before waking the worker | `3651888617` | `crates/phoenix-ide/src/runtime/executor.rs` | `2cdc9fb2f6b9` |
| P2 | Record the shipped wake status and cancellation surfaces | `3653198296` | `specs/wake-contracts/executive.md` | `54497f7d02da` |
| P2 | Complete CLI status before enabling durable waits | `3653261929` | `crates/phoenix-ide/src/api.rs` | `092fd748ae1d` |
| P2 | Document synchronous wait cost and wake cancellation | `3653548514` | `crates/phoenix-tools/src/wait_until.rs` | `9e9265c6871b` |
| P2 | Cap rendered wake results before persistence | `3653592989` | `crates/phoenix-ide/src/runtime/wake.rs` | `59147ff05a95` |
| P2 | Thread wake identity fields into the frontend status model | `3653592992` | `crates/phoenix-ide/src/api/handlers.rs` | `59147ff05a95` |
| P2 | Admit the waiter-panic reason in the receipt schema | `3653662166` | `crates/phoenix-ide/src/runtime/wake.rs` | `ed26fefae85f` |
| P2 | Remove the stale status and cancellation summary | `3654197917` | `specs/wake-contracts/executive.md` | `389755cde18d` |
| P2 | Serialize forgotten reasons with normative wire spelling | `3660670503` | `crates/phoenix-ide/src/runtime/wake.rs` | `be251aefc584` |
| P2 | Release observation authority after inspection errors | `3662614701` | `crates/phoenix-ide/src/runtime/wake.rs` | `46a4fde1e7f2` |
| P2 | Reconcile the wake delivery requirements | `3662614705` | `specs/wake-contracts/requirements.md` | `46a4fde1e7f2` |
| P2 | Carry contract identity in every queued observation | `3662614707` | `specs/wake-contracts/wake-contracts.allium` | `46a4fde1e7f2` |
| P2 | Refresh wake status when SSE lifecycle events arrive | `3679027162` | `ui/src/hooks/useConnection.ts` | `d17a465c53c9` |
| P2 | Accept the specified zero-second wake deadline | `3679172230` | `crates/phoenix-tools/src/wait_until.rs` | `0fd3f8781c43` |
| P2 | Store forgotten reasons with the specified spelling | `3679258254` | `crates/phoenix-db/src/migrations.rs` | `2665aaca9e50` |
| P2 | Retry transient tmux cleanup inspection failures | `3679779589` | `crates/phoenix-tools/src/tmux/run.rs` | `ebb412bf8a7f` |
| P2 | Remove kill-pending from terminal wake delivery | `3679873559` | `specs/wake-contracts/requirements.md` | `2566e16519cb` |
| P2 | Add the wake events to the SSE replay contract | `3680169065` | `crates/phoenix-ide/src/api/wire.rs` | `8ba812ca6d9d` |

## Exact commit-retention map

The disposition is intentionally conservative: tests and types may be extracted; production implementations are design input until proven compatible with the new aggregate.

### Authority-model input (44)

| Commit | Subject |
|---|---|
| `cf5a15c6472bb183c4079aacf62b489f48d7fb4b` | tasks: plan model-first wake replacement stack |
| `8d48b05a339f175c188964cff3060fe3e8753da6` | fix: retry wake checkpoint at persistence boundary |
| `a1b4203bfcdbb25e8d4d9b0ce30131b895df10ff` | fix: drain wake debt before direct turns |
| `006fcf9b0309db7461349dde2f50d63f87fde4f1` | fix: close explicit wake terminal edge cases |
| `994787ee0389695228e3fb2e6a8e075ef1223f0c` | fix: retry consumed wake checkpoint outcomes |
| `36350a9a407ea7d1f67bd3aa1acf2aef448804f4` | fix: fence explicit wake activation publication |
| `2665aaca9e505ad0d767c1c9d2bfbba6f92f0801` | fix: accept immediate explicit wake expiry |
| `d17a465c53c9d783fe022c71f4235309d4b06b3c` | fix: clean up failed explicit wake admission |
| `43187c57884f69a805c893de590cbaf15cefcfc1` | fix: preserve parked wake cancellation after rebase |
| `95b4e27ccfb891ec38be6b1f25a2d812b0b31bfb` | fix: close wake observation leases on errors |
| `55e40c95479c5e3e540f625047d8c72121e569c8` | fix: release failed wake observations promptly |
| `505e7df5abf575614200f73de8c5f77eb4cdc2cd` | fix: preserve wake evidence deadline precision |
| `4ffef02f2b37e9fb4cad28e372703979b19900a3` | fix: scope wake admission to affected conversations |
| `116e7b97bf11d60853aeb44b457238b48c71ae31` | fix: make wake adoption atomic and batch-bounded |
| `36c84fe91d1060f7b0d556112c55d57f9bcd7f01` | fix: preserve durable wake ordering and admission bounds |
| `54cdc5d77600345cef40b1bdfa52f8d46b615514` | fix: retry wake cleanup and scope admission fences |
| `2dff493932400868c1b79b7caa012b6adb1a0162` | fix: preserve bounded wake delivery semantics |
| `6891679087472ec52c310915783edb2d7004974e` | fix: serialize wake lifecycle admission |
| `f8367a7dabd694342c492d14c40b859da4c6041e` | fix: recheck direct turns under wake adoption lock |
| `688fb60d5ddb2a9e0ff3b0badc4e2788fc87a25a` | fix: preserve wake ordering after durable rebase |
| `50d3dce61bc5aff0dc6801bc5b75e287fcfe46b1` | fix: align wake ordering with durable turns |
| `cbd59d1df3279de7bb5b36a9c81eaf0771def671` | fix: fence wake registration during lifecycle teardown |
| `f8bdb1462e0652be9c8c7574ba5a6c2b7657eebe` | fix: close wake registration and delivery races |
| `4b1fc4cdc0336ce238d00ad74bedf5142b902c15` | fix: project wake tool-round identity consistently |
| `7c866febf0848723543cdc84817c6c1f55052031` | fix: preserve wake batches and primary cancellation |
| `65100f37095712a004c6e8fa084614ab05b8554d` | fix: keep wake recovery subordinate to sibling failures |
| `8a1510456b6788e019c7ab7f7c1ae88dc3efcd44` | fix: align wake recovery with durable runtime authority |
| `e6f2f3fe2bc976f11f3aad5373724ede852957bc` | fix: gate wake parking and scope promotion |
| `c2f0f35ee2438f19cebcb3265a9b2307b57395d6` | fix: complete wake contract lifecycle surface |
| `edca38c6e54868da761f072023ae4f0839438354` | refactor: integrate wake and metrics tool context |
| `1294432da6002d61a38441e2cc05f698b4c5dbcd` | fix: complete wake activation and delivery batches |
| `761e5444d846b6d8c3282308f12c4303415032cd` | fix: preserve kill-pending wake timestamp shape |
| `eff4709ef685e479171863d766e2b3856e7a605d` | fix: preserve wake retry and recovery semantics |
| `04e7e48dbafcb066303690debf7683c62a9fd80a` | fix: close wake durability gaps |
| `42bcfaf6e6afdd5303d548c2063b26735a90d227` | fix: correlate owed wakes to wait receipts |
| `9bdaa28df93f4b0aa37bd9d5aa3962067a3f5a96` | fix: resume from adopted wake batch decision |
| `f9561ed1b8256d688f83eabc660706852d1c11b0` | fix: preserve parked wake lifecycles |
| `ca8e6dc480df8186bfecc9df6bb7f5515544fe15` | fix: make wake adoption runtime-atomic |
| `af37ec4f7ac1cee96ff53d8068d909faa737eda9` | fix: publish wake registration after checkpoint |
| `65e9ac0c2e787f5d017d1c211569534a599086db` | fix: harden wake expiry and transfer races |
| `6f984449da754d38c0257fcb1741370bd9f2ee18` | fix: close wake persistence race windows |
| `d5db555d988e0f5ba4274a14d4692e059c1c06e4` | fix: close explicit wake lifecycle gaps |
| `21f80f41298aaed05b0a9fa15f7cccd92afde8ae` | fix: make explicit wake recovery replay-safe |
| `ad3aac0acd869725433a3f96f47cf77dfb26c540` | fix: preserve explicit wake parking semantics |

### Bash adapter input (7)

| Commit | Subject |
|---|---|
| `c1fc25e2743aebf0ad0b1d9fee73d4fa5b45ff15` | fix: align explicit Bash wakes with resource scopes |
| `dc470899bf85e10e14179505f01d5b8d74d1c6fa` | fix: preserve Bash terminal metadata across wake delivery |
| `c2e39b1eece2ff120f157011c5d292e8df190979` | fix: render cancelled bash wake results |
| `d214623251c7983294e7f0e71fb13f3a6652ac58` | fix: render bash wakes as wait observations |
| `779a702989e1544f39c5d30f2fe47b22b0a5cb94` | fix: project kill-pending Bash wakes |
| `6a147e93cbb724e505b90570918f656453c4fd2d` | feat: park explicit Bash wake registrations |
| `ac420a48fe69351f86a31410458e1faca0fda6c4` | feat: add wait_until tool registration surface |

### Projection slice input (6)

| Commit | Subject |
|---|---|
| `ebb412bf8a7f27215dcb095cd2ffd55d8d3adff6` | fix: order wake status refresh and tail suffixes |
| `0fd3f8781c43479b12eeed2bb5508fd5c1a4d872` | fix: refresh wake status from lifecycle events |
| `2e3f1942879a98fbc5f9c865cef00dac6c696646` | fix: normalize wake reason wire encoding |
| `4c18e9129f7ea2e5ab641387a7b01666d5b24ef8` | feat: expose wake handle identity in CLI status |
| `51b4a3485fb28aedb82f34282b67c1188bb82637` | fix: preserve wake cancellation and SSE ordering |
| `7a3f3b6c5c214139773ba827f2eb9ec431b61712` | build: normalize generated TypeScript whitespace |

### Extract tests/evidence (6)

| Commit | Subject |
|---|---|
| `0ff89e59d043b47f70532be13511169fea4f5b94` | test: inspect registered tmux server socket |
| `616caa3fcf2d45efe8246949f8ad9a9435b63ff6` | test: inspect the work-scope tmux socket |
| `1a5ec651171aa30505c1ac6966b1ef3e14fa5aaf` | test: type wake status fixture |
| `90888fb93928b15fe7bd92ea374ddadfa59c8968` | test: distinguish wake retries from sibling waits |
| `88def18a5ef646916a29a6bc68abdd26f1a47753` | test: bind wake activation fixture to durable scope |
| `add1a957e0da425091c59079956dc89a67276b8f` | fix: repair phoenix-state-machine test fixtures |

### Model/spec input (2)

| Commit | Subject |
|---|---|
| `bc0e19e354877a5c114ac7bff9167dfa6d3a2d96` | docs: reflect shipped wake status surfaces |
| `632dc0a32b6fd271570a42388baf5724f7011513` | docs: record Bash wait_until slice |

### Defer to TmuxWindow slice (6)

| Commit | Subject |
|---|---|
| `2566e16519cb41ee44e2d1651ec9f615adf9e1bd` | fix: retain uncertain wake and tmux observations |
| `7b51ae17d8824df48f32e689dcac98ebeb4196b3` | fix: fence readiness cleanup by tmux identity |
| `0f0aa34801ae24b3252b196c29f847b38e69bdc4` | fix: preserve terminal wake and tmux completion states |
| `cf8d02dc329eae45c574470db5831207d2410865` | fix: serialize turn acceptance and remove implicit tmux wakes |
| `d5029abd3e487f66dd7968eb854941568b7476e2` | fix: remove stale tmux expiry calculation |
| `fa9a7c0e92bfbc026cf3c2ec97a5848d98f88511` | refactor: limit tmux changes to wake adapter compatibility |

### Compatibility/infrastructure; cherry-pick only if independently needed (4)

| Commit | Subject |
|---|---|
| `35e734b0c971629997fad6880666a0f774f4a233` | checkpoint: preserve waiter panic occurrence time |
| `8ba812ca6d9d14e2e5a3af78173d8dec9a72f8be` | fix: preserve waiter panic evidence and cleanup verification |
| `f49488938b9f0e7d352535475949e49e00b0beb1` | fix: close readiness windows after completion |
| `91c4f65d029780e45f3d35446f65d759ffc95e5f` | tasks: track host-load check flakes |

## Downstream decision gate

Completion of this foundation is not approval to build the full wake roadmap. Downstream implementation remains paused by default.

A Bash-only integration proposal may proceed for review only when it can answer all of the following without changing the foundation's authority model:

1. **User value:** name the concrete polling journey being replaced and the terminal result the original tool invocation must receive.
2. **Capability origin:** identify the existing Bash resource owner that mints registration and observation capabilities; no public constructor or caller-supplied handle description substitutes for ownership.
3. **Single transaction path:** use the wake repository for registration, observation, cancellation, expiry, and terminal acceptance; do not reopen generic workflow mutation paths.
4. **Restart behavior:** accept that an in-memory Bash handle becomes `Forgotten(PhoenixRestart)` and prove the result is delivered rather than silently lost.
5. **Bounded proof:** add one end-to-end Bash journey and only the adapter-specific regressions selected from the historical matrix; do not add UI, tmux, or sub-agent abstractions.
6. **Complexity budget:** show that the adapter is substantially smaller than the foundation and does not introduce a second scheduler, authority record, delivery queue, or continuation protocol.

UI/projection work remains paused until the Bash journey proves a durable status is needed by users. Tmux and sub-agent adapters remain paused until Bash demonstrates that the common contract abstraction pays for itself without substrate-specific exceptions. Continuation transfer should be evaluated as a separate product decision, not assumed by the first adapter.

Absent that evidence, the recommended outcome after this task is to merge the bounded foundation as documented design and keep tasks 44012–44014 blocked rather than treating sunk implementation cost as justification for the platform.

## Foundation acceptance

- The concrete user problem, harmful failure classes, foundation boundary, and non-goals are explicit.
- The authoritative aggregate and repository are the only semantic mutation boundary; adapters, generic workflow primitives, and projections cannot become parallel authorities.
- Cancellation is observation-only, and its occurrence-precedence/fence/settlement policy is stated independently of any substrate adapter.
- Pure transition tests cover arbitrary command sequences, replay, stale generations, crossed authority, and exactly one closed lifecycle.
- Repository tests cover atomic terminalization, exact owed-delivery recovery, fence rollback, cancellation settlement, restart reload, and transactional workflow-ID allocation.
- Migration tests prove fresh and upgraded databases enforce the same closed schema and nominal types reject invalid values before SQL.
- The twenty-six Codex foundation findings are mapped to decisions and evidence; a fresh exact-head Codex review must produce no actionable thread (👍).
- No Bash, tmux, sub-agent, router, SSE, UI, transcript, or conversation-state integration is included.
- Downstream work is governed by the explicit decision gate above and remains paused unless a bounded Bash proposal satisfies it.
