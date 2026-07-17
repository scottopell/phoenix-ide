# Grand vision review: durable workflows grounded in Phoenix’s deployment reality

## Purpose

Adversarially review the durable workflow runtime stack culminating in PRs #488 and #485 while treating Phoenix’s long-term reliability vision as a real product constraint—not as overengineering by default. Determine the smallest durability model that makes the required incorrect states structurally unrepresentable for Phoenix’s actual deployment target: one API-server process with bundled SQLite.

The review must distinguish:

- essential correctness mechanisms,
- accidental complexity,
- complexity justified only by plausible future capabilities,
- and abstractions that prematurely encode an uncertain future.

This is an architecture and product-direction review. The user is the oracle for intended semantics; unresolved product assumptions must be surfaced as concrete choices rather than silently inferred.

## Investigation

1. Use `gh` with network access to inspect the full stacked change set around `feat/workflow-persistence-engine-wake`, especially PR #488 and stack-tip PR #485.
2. Read all useful review discussion on #485, including inline comments, review threads, resolved conversations where available, issue comments, commit history, and linked context. Inspect the corresponding material on #488 and other stack PRs as needed.
3. Read the relevant workflow, persistence, runtime, coordinator, and recovery specifications and ADRs before judging implementation choices.
4. Map the implementation’s actual state transitions, transaction boundaries, wake/recovery paths, ownership boundaries, and failure handling.
5. Compare that model against concrete failures possible in a single-process SQLite deployment:
   - process crash or kill at each persistence/effect boundary,
   - restart and replay,
   - duplicate delivery or duplicate execution,
   - concurrent requests/tasks within one process,
   - SQLite transaction failure, lock contention, or disk failure,
   - cancellation and shutdown,
   - partially completed external I/O,
   - stale wakeups and orphaned work,
   - schema/version evolution.
6. Separately analyze anticipated scheduled loops, global-coordinator interaction, remote executors, SSH/container environments, and broader I/O-boundary conversion. Do not credit complexity merely because it could hypothetically support these features; identify which future requirements are sufficiently concrete to constrain today’s design.

## Root-invariant analysis

Derive and validate a compact set of root invariants. At minimum, investigate whether the system truly needs guarantees equivalent to:

- durable intent exists before externally observable work begins,
- each persisted workflow has one authoritative state and recoverable owner,
- restart cannot lose acknowledged work,
- replay cannot silently duplicate non-idempotent effects,
- state advancement and wake eligibility cannot contradict each other,
- terminal workflows cannot become runnable again without an explicit legal transition,
- retries and cancellations have explicit, non-overlapping semantics,
- unsupported durability guarantees are visible rather than implied,
- and persistence representation prevents—not merely documents—invalid states.

Challenge each proposed invariant: identify its user-visible consequence, the failure that violates it, and whether SQLite plus a single process already supplies part of the guarantee.

## Oracle checkpoints

Pause for focused discussion with the user when product intent changes the architecture. Present concrete options and consequences, especially for:

- the exact acknowledgment/durability contract exposed to users,
- crash semantics around non-transactional external effects,
- acceptable at-least-once versus effectively-once behavior,
- whether workflows may survive binary/schema upgrades in flight,
- scheduling guarantees and missed-run behavior,
- coordinator authority and ownership,
- remote executor trust, lease, connectivity, and partition assumptions,
- and whether future multi-process or distributed operation is a goal, an option to preserve, or explicitly out of scope.

Do not collapse these into generic “future scalability” arguments.

## Adversarial comparison

Develop at least three coherent models and compare them against the same invariant/failure matrix:

1. the implementation in the PR stack,
2. a minimal single-process + SQLite durable runtime,
3. a justified middle path that preserves only the seams needed by credible near-term features.

For each model, assess safety, liveness, recovery clarity, representable invalid states, operational debuggability, migration burden, implementation cost, and feature-development tax. Prefer deleting machinery when an existing SQLite transaction, uniqueness constraint, typed state transition, or process-ownership rule provides the same guarantee.

## Deliverables

Produce a review package containing:

- a concise architecture map of the stack,
- a root-invariant and failure-mode matrix,
- findings ranked by severity and confidence with direct code/PR/spec evidence,
- an explicit accounting of essential versus speculative complexity,
- a recommended durability model for the current deployment target,
- a forward-compatibility analysis for scheduled loops, the global coordinator, and remote executors,
- concrete keep/simplify/delete/defer recommendations,
- and a sequenced path to finish or reshape the stack without destabilizing already-earned correctness.

Where appropriate, prepare actionable GitHub review comments, but do not post comments or change code until the user has reviewed the architectural conclusions. Clearly separate confirmed defects, invariant gaps, simplification opportunities, and unresolved product decisions.

## Review standard

Be helpfully critical in both directions. “Distributed-systems machinery in a single binary” is not itself a valid objection, and “future remote execution” is not itself a valid justification. Every mechanism must pay rent through a named invariant or a sufficiently concrete capability. Every proposed simplification must preserve the reliability property that the removed mechanism actually supplied.
