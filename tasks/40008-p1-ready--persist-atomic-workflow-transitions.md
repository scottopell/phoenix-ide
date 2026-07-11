# Persist Atomic Workflow Transitions and Effect DAGs

## Mission

Implement normalized SQLite persistence and production-neutral storage APIs for the Durable Workflow Runtime. A workflow transition, its append-only history, all declared effect intents/dependencies, and its completion barrier must commit atomically under workflow-version CAS.

Depends on:

- **Specify the Durable Workflow Runtime**
- **Build the Pure Durable Workflow Engine and Simulator**

Read those tasks’ normative specs and reuse the pure engine’s types/decisions. Do not create a second persistence-specific protocol model.

## Data model

Implement normalized relational storage for:

- Workflow current snapshots: ID, kind, version, generation, state discriminator/payload, timestamps
- Append-only workflow transitions by workflow/version
- Effect intents: family, kind, codec version, requirement, recovery capability, status, durable deadline
- Effect dependencies as rows
- Effect attempts: attempt number, worker, token, lease, timing
- Effect observations
- Typed receipts
- Completion barriers and required-effect membership

Queryable authority/scheduling fields must be columns, not JSON paths. Polymorphic intent/observation/receipt payloads may use earned whole-value blobs with versioned codecs. Child collections such as dependencies and barrier members must be rows.

## Atomic APIs

Provide typed APIs conceptually equivalent to:

```rust
commit_transition(expected_version, transition_plan)
claim_eligible_effect(worker, token, now, lease)
renew_effect_claim(authority, now, lease)
record_observation(authority, observation)
record_receipt(authority, receipt)
schedule_effect_retry(authority, deadline, reason)
commit_receipt_transition(expected_version, receipt_event_plan)
cancel_with_compensation(expected_version, cancellation_plan)
next_effect_deadline(now)
```

Expected races return typed outcomes such as:

```rust
Applied
VersionConflict
AuthorityLost
NotEligible
```

Do not surface normal authority loss as a user-visible workflow failure.

## Required transaction boundaries

- Transition snapshot + history + intents + dependencies + barrier: one transaction.
- Receipt persistence + any barrier update: one transaction.
- Cancellation state/generation + old-claim revocation + compensation intents/barrier: one transaction.
- Any workflow mutation requiring authority must include its predicate in the same SQL statement/transaction.
- Terminal completion must verify its required barrier inside the completing transaction.

## Scheduler semantics

`next_effect_deadline` must reflect the earliest time work can become claimable:

- Retry deadlines
- Active claim lease expiry
- Manual-resolution wakeups where applicable
- Dependency changes should wake through committed notifications/kicks, while durable deadlines guarantee recovery after lost kicks

Avoid zero-delay hot loops where a row is not claimable until a later lease/deadline.

## Schema correctness

Use SQLite constraints for lifecycle shape:

- Status-specific required/forbidden timestamps
- Claim fields all present or all absent
- Retry deadline iff retry-scheduled
- Generation/version nonnegative
- Unique workflow/version transition
- One receipt per effect
- Dependency foreign keys and no self-dependency
- Barrier membership references same workflow/transition

DAG acyclicity may require transactional validation in Rust unless a practical schema constraint exists; it must still be tested.

## Migration and startup

Follow Phoenix’s actual base-DDL plus numbered-migration startup behavior. Ensure fresh databases, upgraded databases, and reopened migrated databases all work. Base DDL must remain rerunnable against the migrated schema or startup must be deliberately redesigned and specified.

Do not migrate existing conversation-creation jobs into the new engine in this task. New tables may coexist unused until shadow adoption.

## Testing

Use real SQLite files/connections, including contention tests:

- Two writers race the same expected workflow version; one wins.
- State and effect intents never commit partially.
- Two workers race one eligible effect; one claim wins.
- Lease expiry takeover increments attempt/authority correctly.
- Stale observation, retry, receipt, and completion writes are rejected.
- Dependencies block eligibility until compatible receipts exist.
- Required barrier completion is exact.
- Optional receipt does not gate completion.
- Cancellation atomically revokes claims and appends compensation.
- Old-generation receipts cannot alter current state.
- Durable retry/lease deadlines wake correctly without a kick.
- Fresh migration, upgrade, and reopen all pass.
- Codec-version rejection/migration behavior is explicit.

## Non-goals

- Executing external effects
- Creation shadow adapter
- Migrating or deleting current creation tables
- UI/API changes
- Full event sourcing
- Runtime-wide cutover

## Acceptance criteria

- [ ] Normalized schema and migrations exist with constraints.
- [ ] Atomic transition commit uses pure engine plans and workflow-version CAS.
- [ ] Claims, leases, generations, retries, receipts, barriers, and cancellation have typed APIs/outcomes.
- [ ] No SQL scheduling logic duplicates pure rules without parity tests.
- [ ] Real contention, crash-window, migration, and deadline tests pass.
- [ ] Existing production workflows remain behaviorally unchanged.
- [ ] Full project checks pass.

## Follow-up dependency

The next task is **Shadow Conversation Creation on the Durable Workflow Runtime**.
