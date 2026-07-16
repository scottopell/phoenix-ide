# Unify wake and conversation creation on a durable workflow runtime

Build one shared durable workflow engine from the best proven ideas in the conversation-creation protocol and wake-plane prototype. Hold PR #471 until wake runs on the shared engine; wake is the first production adopter, and conversation creation follows through shadow parity and versioned cutover.

This umbrella task is the sole authority for program sequencing, architecture ownership, migration gates, and release criteria. It supersedes conflicting sequencing or ownership in tasks 40006–40011 and 47002. Those tasks remain normative inputs and narrower acceptance references until transitioned below.

## Product outcome

Phoenix users can start asynchronous work and trust that accepted obligations survive crashes, retries, takeovers, cancellation, continuation, and process restart without duplicate external effects or lost delivery.

The first delivered journey remains wake:

```text
agent registers bounded bash/tmux wait
→ provider-valid receipt commits
→ workflow observes terminal evidence under a lease
→ durable receipt reaches the reducer
→ conversation resumes exactly once when eligible
```

Conversation creation then adopts the same engine without losing shell-first acceptance or its existing fenced reconciliation guarantees.

## Settled program decisions

1. **Do not land PR #471 independently.** Preserve it as a behavioral oracle while replacing its bespoke scheduling authority.
2. **Land a reviewable stack:** unified specification; pure engine plus typed wake profile; persistence plus engine-backed wake; creation shadow; creation cutover.
3. **Wake is the first engine adopter.** Production wake lands only with shared persistence and engine-backed scheduling; creation shadow/cutover follows.
4. **Specify a general engine through two concrete normative profiles:** wake and conversation creation. Future profiles are named follow-ups, not speculative requirements.
5. **Lease every claimed external-effect step.** Every executor operates under workflow version, generation, claim token, and lease authority; stale results cannot commit.
6. **Use engine-owned normalized execution tables plus typed profile tables.** Core owns execution truth; profile tables own non-overlapping domain intent/projections.
7. **The existing product reducer remains the sole owner of product meaning.** The workflow engine persists and executes transition plans; receipts return through the same reducer contract.
8. **Observation, receipt, and runtime acceptance are distinct durable concepts.** External evidence is not automatically product success; product meaning is reducer-owned.
9. **Kicks are latency optimizations.** Durable deadlines and claims guarantee progress after missed signals or restart.
10. **No universal exactly-once claim.** Every effect family declares one ambiguity policy: observable reconciliation, external idempotency, safe repeatability, or manual resolution.

## Architectural union

### Keep from durable conversation creation

- generation, claim-token, and lease fencing;
- inspect-before-replay reconciliation;
- explicit durable retry deadlines;
- normalized resource reservations;
- immediate visible cancellation with authority revocation;
- deterministic fault schedules and takeover tests.

### Keep from wake contracts

- immediate receipt followed by later runtime observation;
- observation-before-acceptance discipline;
- durable inbox/outbox acceptance boundary;
- deterministic, coalesced resume delivery;
- continuation-safe transfer of unconsumed obligations;
- cancellation arbitration that cannot hide an auto-resuming result;
- lifecycle fencing and restart-durable bash/tmux evidence.

### Replace

- creation stages as an implicit effect graph;
- wake tables as a permanent independent scheduler;
- DB helpers that directly decide product completion/failure;
- hidden cleanup status conventions instead of compensation effects;
- domain-specific hot polling as a correctness mechanism;
- duplicated scheduling, lease, retry, and acceptance machinery.

## Engine-owned normalized core

The normative spec must define normalized tables equivalent to:

- workflow snapshots: identity, profile/version, reducer state discriminator/payload, workflow version, generation, timestamps;
- append-only transitions: event family/kind/version, payload, committed workflow version;
- effect intents: family/kind/version, required/optional/compensation role, ambiguity policy, status, generation, durable deadline;
- effect dependencies;
- live claims: worker, token, generation, lease;
- append-only attempts;
- append-only observations;
- one accepted terminal receipt per effect;
- completion barriers and required membership;
- reducer inbox / owed runtime acceptance where a durable receipt must later enter a product runtime.

Authority and scheduling fields must be queryable columns. Typed polymorphic payloads may be earned blobs only when read and written whole. Core and profile tables must never represent the same semantic value twice.

## Profile boundary

The engine owns identity, versions, generations, claims, leases, dependencies, deadlines, attempts, observations, receipts, barriers, and scheduling.

A registered profile owns:

- typed workflow state and event codecs;
- typed effect/observation/receipt families;
- external inspection and execution adapters;
- reconciliation and ambiguity policy;
- physical resource locks;
- compensation meaning;
- mapping receipts back into the product reducer;
- derived user capabilities and projections.

## Milestone 1 — unified normative specification

Create `specs/durable-workflows/` as the umbrella normative home. Requirements and Allium must define the general engine and concrete wake/creation profiles without time-relative rollout prose.

Resolve explicitly:

- workflow-version CAS versus per-step lease authority;
- observation → receipt → reducer-acceptance boundaries;
- typed effect DAG declaration and barrier satisfaction;
- claim, renewal, expiry, takeover, and stale-result rejection;
- durable scheduler deadlines;
- cancellation generation bump and compensation DAG declaration in one commit;
- manual-resolution state and capabilities;
- core versus profile table ownership;
- protocol/family codec versioning;
- shadow/non-authoritative workflow representation;
- wake migration and creation migration boundaries.

Write new ADRs rather than rewriting ADR-007 through ADR-012. Align wake, creation, and bedrock specs. Run the full spec-authoring preflight.

### Milestone 1 acceptance

- Wake and creation can both be represented without domain-specific exceptions in core contracts.
- Every claimed step is lease-backed by normative rule.
- The product reducer remains the only product-semantic authority.
- No artifact retains creation-first or wake-only architectural authority.
- Allium and spec checks pass with no relevant errors.

## Milestone 2 — pure engine and deterministic simulator

Implement a side-effect-free engine:

```text
transition(snapshot, typed event, profile context)
→ next snapshot
→ append-only transition
→ declared effect DAG
→ cancelled effects / compensation DAG
→ barriers
→ derived capabilities
```

Provide typed profile adapters for wake and creation. Generate schedules covering:

- duplicate kicks and concurrent workers;
- lease renewal/expiry/takeover;
- stale and late results;
- ambiguous external outcomes;
- retry deadlines;
- cancellation/deletion during every step;
- compensation failure and takeover;
- wake coalescing, continuation transfer, and acceptance;
- creation shell acceptance, reconciliation, and cleanup.

### Milestone 2 acceptance

- Persistence can consume engine plans without inventing business rules.
- Deterministic simulation proves stale authority cannot commit.
- Required barriers, not ad hoc writes, derive engine completion truth.
- Wake and creation schedules pass the same generic claim/retry/cancel properties.

## Milestone 3 — atomic persistence and durable scheduler

Land engine-owned normalized tables and transactional APIs.

One reducer commit atomically writes:

- guarded next workflow snapshot/version/generation;
- append-only transition;
- declared effects and dependency edges;
- barriers and required members;
- cancellation/compensation effects when applicable.

Implement claim/renew/observe/receipt/takeover APIs and a scheduler driven by the earliest durable deadline across eligible effects. Kicks only accelerate scans.

### Milestone 3 acceptance

- A failed transaction leaves no partial transition/effect graph.
- Claim authority is structurally checked on every observation/receipt commit.
- Scheduler restart recovers all owed work from rows and deadlines alone.
- No authority query requires JSON extraction.
- Creation and wake legacy paths remain authoritative; new engine tables are inert or shadow-only.

## Milestone 4 — migrate wake onto the shared engine

Use wake as the first authoritative profile while preserving its current user contract.

Recommended migration:

1. Map wake registration to workflow creation and initial effect DAG.
2. Run the existing wake path and engine profile in shadow; persist divergences.
3. Feed authoritative bash/tmux observations into the shadow profile.
4. Prove parity for terminal cause, deadline precedence, cancellation arbitration, coalesced delivery, continuation transfer, lifecycle fencing, and resume acceptance.
5. Cut new wake registrations to engine authority behind a protocol/version boundary.
6. Drain pre-cutover wake obligations on the legacy path; do not reinterpret mixed-authority rows in place.
7. Remove or demote duplicated wake scheduler/claim/outbox mechanics only after drain.
8. Land production wake only after the engine-backed acceptance gate below passes.

Wake profile tables may initially retain wake-specific intent, terminal payloads, and normalized tails. Generic claims, attempts, deadlines, observations, receipts, and acceptance bookkeeping belong to engine core.

### Milestone 4 acceptance

- Provider-valid registration receipt commits before parking.
- User chat remains available while waits are pending.
- Bash and tmux waits survive restart according to substrate capability.
- No overlapping LLM request or lost/duplicate resume.
- Cancellation cannot hide an auto-resuming result.
- Continuation transfers unconsumed delivery obligations without changing resource identity.
- Lifecycle fences remain race-safe.
- Shadow parity has zero unresolved semantic divergences.
- A reversible pre-merge cutover flag/protocol boundary exists.
- Full repository checks and independent review pass.

## Milestone 5 — conversation creation shadow

Implement a creation profile over the same engine while the existing creation protocol remains authoritative.

Model at minimum:

- repository resolution;
- worktree reservation/materialization/reconciliation;
- attachment finalization;
- metadata commit;
- initial-message expansion;
- runtime bootstrap and first LLM dispatch;
- cancel/delete compensation DAGs.

Shadow execution must be structurally unable to perform external side effects. Mirror authoritative observations/receipts into shadow workflows and persist typed divergences.

### Milestone 5 acceptance

- Shell-first acceptance is unchanged.
- Shadow cannot mutate Git, filesystem, runtime, provider, or user-visible state.
- Deterministic crash/takeover campaigns produce semantic parity.
- Every divergence is fixed or resolved as an explicit normative decision.
- Shadow overhead is measured, bounded, and disableable.

## Milestone 6 — creation cutover and legacy drain

Stamp new creation acceptances with a protocol version selecting engine or legacy authority. Cut new jobs to engine authority only after shadow parity. Existing jobs remain on their original authority until terminal.

Rollback routes only new acceptances back to legacy. Never translate an in-flight engine job backward or an in-flight legacy job forward.

### Milestone 6 acceptance

- Creation remains immediate-shell-first.
- No duplicate Git/worktree/provider side effect under replay or takeover.
- Cancel, delete, and retry semantics remain user-compatible.
- Engine-backed and legacy jobs drain independently and observably.
- Legacy worker/tables are removed only after zero-authority drain is proven.

## Later adoption

Task 40011’s runtime-wide adoption remains downstream. Approval, continuation, completion, cleanup, task lifecycle, and tool execution become separate profile-adoption tasks after wake and creation prove the engine. Do not absorb them into the first implementation.

## Task authority transitions after approval

On approval of this umbrella task:

1. Mark this task `in-progress`.
2. Mark task 40006 `wont-do` as superseded by Milestone 1 while retaining its body as design input.
3. Mark tasks 40007 and 40008 `blocked` on Milestone 1, then revise them into child implementation tasks for Milestones 2 and 3.
4. Mark task 47002 `in-progress` or `blocked` as the held wake adopter, with completion redefined by Milestone 4 rather than the current bespoke implementation.
5. Mark tasks 40009 and 40010 `blocked` on the wake adoption gate and revise them into Milestones 5 and 6.
6. Keep task 40011 `blocked` on Milestone 6.
7. Preserve task 54007 as the later sub-agent wake-handle follow-up; do not add sub-agents to the first shared-engine slice.

Task filenames remain the source of status truth. Bodies should identify this umbrella task as sequencing authority without rewriting historical completed-task context.

## Migration and rollback principles

- Use explicit protocol/version boundaries.
- Do not reinterpret in-flight rows under a new authority model.
- New registrations/acceptances select one authority exactly once.
- Old authority drains its own work.
- Rollback changes authority for new work only.
- Shadow mode cannot produce external effects.
- Dual-write may record parity evidence, but semantic authority must be singular.
- Remove legacy paths only after durable zero-authority proof.

## Anti-goals

- No wake-only or creation-only “shared” engine.
- No big-bang wake + creation cutover.
- No generic event-sourcing system; use normalized snapshots plus append-only transition/attempt/observation/receipt history.
- No parallel authoritative reducer.
- No central monolithic enum for every future effect; use registered typed families and codec versions.
- No persisted duplicate capabilities or semantic status mirrors.
- No JSON blobs for queryable authority, deadlines, dependencies, claims, or barriers.
- No plugin marketplace or arbitrary third-party workflow API in this program.
- No sub-agent wake integration in the first adoption slice.
- No production wake landing before the engine-backed wake gate passes.

## Verification

Every milestone must run its focused tests and `./dev.py check`. Specification changes require the `specs/AUTHORING.md` preflight. Persistence and adoption milestones require deterministic crash-boundary and concurrent-worker tests, migration idempotency, restart recovery, and independent review before cutover.


## Umbrella authority and dependency

Task 47003 is the sole authority for shared-engine ownership, sequencing, migration gates, and release criteria. This task preserves its historical design context and narrower acceptance detail, but any conflicting creation-first, wake-only, bespoke-scheduler, or rollout direction is superseded. Implementations SHALL follow `specs/durable-workflows/requirements.md` and ADR-013 through ADR-016.

Milestone 1 normative artifacts are `specs/durable-workflows/requirements.md`, `specs/durable-workflows/durable-workflows.allium`, the profile Allium specifications, its executive summary, and ADR-013 through ADR-016.
