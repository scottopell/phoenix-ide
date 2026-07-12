# Build the Pure Durable Workflow Engine and Simulator

## Mission

Implement a dependency-light pure model of the Durable Workflow Runtime plus deterministic virtual-time simulation. Do not integrate SQLite, Git, axum, provider clients, processes, or production scheduling in this task.

This task depends on **Specify the Durable Workflow Runtime**. Read and obey `specs/durable-workflows/requirements.md` and `durable-workflows.allium` before implementation.

## Architectural role

The crate is the executable protocol model and shared vocabulary for later production persistence/execution. It must make illegal workflow/effect states difficult or impossible to represent, and it must provide the same scheduling, fencing, retry, reconciliation, cancellation, and completion rules that production will use.

Suggested crate:

```text
crates/phoenix-workflow/
```

The final crate name may follow workspace conventions, but it must not live inside the creation worker or depend on `phoenix-ide`.

## Fixed contracts

Implement typed forms of:

```rust
WorkflowId
WorkflowVersion
WorkflowGeneration
TransitionPlan
EffectDag
EffectId
EffectFamily
EffectIntent
EffectDependency
EffectRequirement
RecoveryCapability
WorkflowWriteAuthority
LiveEffectAuthority
EffectAttempt
EffectObservation
ReconciliationDecision
TypedEffectReceipt
CompletionBarrier
WorkflowCapabilities
```

The reducer-facing contract is conceptually:

```rust
transition(state, event, context) -> TransitionPlan {
    next_state,
    durable_effects,
    completion,
}
```

Receipts re-enter the same reducer through typed internal events.

## Effect model

Support typed families such as Git, filesystem, LLM, tool, runtime, and notification. The pure crate should define the family/codec contracts but not concrete production handlers.

Every effect declares exactly one:

```rust
ObservableReconciliation
ExternalIdempotency
SafelyRepeatable
ManualResolution
```

Reconciliation decisions must distinguish at least:

```text
Perform
Adopt
Repair or compensate
Durable conflict
Manual resolution
Retry infrastructure failure
Authority lost
```

Do not model universal exactly-once execution.

## DAG and completion

- Validate acyclic dependencies.
- Make nodes eligible only after compatible dependency receipts.
- Permit independent eligible nodes in parallel.
- Distinguish required, optional, and compensation effects.
- Derive completion from typed barriers; reducers/handlers cannot mark completion ad hoc.
- Ensure optional effects may continue after required completion.

## Authority and cancellation

- Serialize reducer commits by expected workflow version.
- Fence effects by workflow generation plus effect claim token/lease.
- Model lease renewal, expiry, takeover, and stale receipt replay.
- Cancellation advances generation, commits cancelled state, revokes old claims, and appends explicit compensation intents atomically in the pure model.
- Old-generation external success must be observable/adoptable by compensation but cannot mutate current workflow state directly.

## Deterministic simulation

Build generated discrete-event simulations with explicit virtual time, multiple workflows, multiple workers, multiple resources, crashes, and scheduler interleavings.

Generated operations must include:

- Commit transition plan
- Competing reducer event/version conflict
- Claim effect
- Renew claim
- Advance virtual time
- Crash worker
- Observe absent/complete/partial/conflicting state
- Start external effect
- Complete effect
- Succeed but lose acknowledgement
- Replay stale observation/receipt
- Schedule retry
- Cancel workflow
- Emit/execute compensation
- Resolve conflict manually
- Complete required barrier
- Run optional effect after completion

## Required properties

At minimum:

- One writer advances each workflow version.
- Committed transition and declared intents are inseparable.
- Stale generations never affect current state.
- At most one live authority exists per effect.
- Dependencies prevent early execution.
- Required barriers never complete early.
- Optional effects never block completion.
- Lost acknowledgements reconcile without blind duplicate execution when observable.
- Effects without an ambiguity policy cannot be registered.
- Cancellation is immediately visible and cleanup remains durable.
- Compensation is explicit and itself recoverable.
- Simulated scheduler progress depends on durable deadlines, not arbitrary sleeps.

Check minimized Proptest regressions into the crate’s regression directory. Treat every discovered counterexample as a protocol bug, not flaky input.

## Capability projection

Define an exhaustive reducer-owned typed capability projection contract. The pure engine need not know conversation-specific states, but it must support domain reducers deriving capabilities without persisting them as parallel state.

## Non-goals

- SQLite schema or migrations
- Production worker loops
- Real Git/filesystem/provider effects
- Creation adapter
- UI integration
- Runtime-wide cutover
- Generic string-keyed plugin execution

## Verification

- Unit tests for all pure transitions and validation rules.
- Proptest campaigns with checked-in regressions.
- Deterministic replay: identical operation history yields identical final snapshot, intents, attempts, receipts, and barriers.
- `cargo test -p <workflow-crate>` and workspace checks.
- Independent review focused on invalid representable states and model/spec drift.

## Acceptance criteria

- [ ] Pure crate exists with no production-effect dependencies.
- [ ] Transition plans, DAGs, authority, recovery policy, receipts, and barriers are typed.
- [ ] Invalid DAGs and missing ambiguity policies are rejected structurally.
- [ ] Virtual-time simulation covers all operations/properties above.
- [ ] Production-facing code can reuse the pure scheduling/retry/eligibility decisions rather than reimplement them.
- [ ] Checked-in regressions document any counterexamples found during development.
- [ ] All tests and full project checks pass.

## Follow-up dependency

The next task is **Persist Atomic Workflow Transitions and Effect DAGs**.


## Umbrella authority and dependency

Task 47003 is the sole authority for shared-engine ownership, sequencing, migration gates, and release criteria. This task preserves its historical design context and narrower acceptance detail, but any conflicting creation-first, wake-only, bespoke-scheduler, or rollout direction is superseded. Implementations SHALL follow `specs/durable-workflows/requirements.md` and ADR-010 through ADR-012.

This task is blocked on task 47003 Milestone 1 and is a child of the pure-engine or atomic-persistence milestone respectively.
