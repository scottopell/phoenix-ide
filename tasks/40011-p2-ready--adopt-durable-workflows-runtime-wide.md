# Adopt Durable Workflows Across Cleanup, LLM, Runtime, Tools, and Notifications

## Mission

Incrementally adopt the Durable Workflow Runtime beyond conversation creation. Each workflow migration is a separate vertical slice with specifications, parity/fault testing, a durable version boundary, and independent review. Do not perform a runtime-wide big-bang rewrite.

Depends on successful conversation-creation cutover and operational confidence in the shared engine.

## Adoption order

Use evidence to adjust order, but default to:

1. Creation cleanup/compensation if not already fully engine-native
2. Initial and subsequent LLM dispatch
3. Runtime lifecycle effects
4. Tool/process execution, including Bash/Tmux ownership where appropriate
5. Notifications, analytics, and optional projections

Create child tasks/PRs for each vertical slice rather than implementing all surfaces in one change.

## Per-workflow migration template

Every adopted workflow must define:

- Domain states and reducer events
- Typed effect DAG
- Required, optional, and compensation effects
- Completion barrier
- Effect intent/observation/receipt codecs and versions
- Explicit ambiguity policy per effect
- Resource-specific locks
- Cancellation and compensation semantics
- Capability projection
- Durable protocol/version boundary
- Legacy drain and rollback strategy
- Deterministic simulator/fault campaigns
- Production shadow/parity evidence

## LLM dispatch considerations

Investigate provider capabilities before choosing recovery policy:

- Externally enforced idempotency where genuinely supported
- Observable request/result identity where available
- Safe repeatability only when semantically true
- Manual resolution for irreversible/unobservable ambiguity

Do not claim exactly once. A broader durable request outbox may emerge from the shared effect engine, but must be runtime-wide rather than creation-local.

Required scenarios include persistence-before-dispatch, dispatch-before-ack, stream interruption, provider retry, cancellation, usage-limit failure, and duplicate-result fencing.

## Runtime lifecycle considerations

Model start, stop, eviction, reconstruction, and publication as typed effects where durability is valuable. Preserve SSE continuity through manager-owned broadcasters. Runtime capability must be reducer-derived; invalid or terminal states cannot start a runtime merely because a caller forgot a guard.

## Tool/process considerations

Before adopting each tool family, classify:

- Process-group/session ownership
- Whether execution is observable after Phoenix restart
- Whether effects are safely repeatable
- Whether cancellation can physically stop the effect
- Required cleanup/compensation
- Durable output/receipt shape

Unobservable orphanable processes may require manual-resolution or stronger supervisor integration before admission to the engine.

## Notifications and projections

Notifications/analytics are generally optional effects and must not block workflow completion. Their retry/duplication policy must be explicit. UI/SSE events are projections of committed state/receipts, never prerequisites for durable progress.

## Cross-cutting cleanup

As adoption proceeds:

- Remove bespoke schedulers only after their workflow drains.
- Remove duplicated claim/lease/retry implementations.
- Replace negative state lists with reducer-owned capability projections.
- Consolidate shared effect families/codecs without making the engine domain-aware.
- Preserve append-only history and codec migration support.
- Keep schema columns normalized for all queried authority/scheduling fields.

## Operational requirements

Add observability for:

- Workflow/version/generation
- Effect DAG and blocked dependencies
- Claims/leases/attempts
- Retry deadlines
- Reconciliation decisions
- Conflicts/manual-resolution needs
- Barrier progress
- Compensation progress
- Shadow/parity divergences

Logs must expose unsupported capability gaps at debug level or above.

## Non-goals

- One PR for all workflows
- Automatic generic rollback
- Arbitrary untyped plugins
- Full event sourcing
- Persisted duplicate capability state
- Deleting legacy paths before drain/rollback criteria are met

## Acceptance criteria

For each child migration:

- [ ] Normative workflow behavior and ambiguity policies are specified.
- [ ] Typed DAG, receipts, barriers, compensation, and capabilities exist.
- [ ] Shadow/parity and deterministic crash campaigns pass.
- [ ] Durable version boundary and rollback plan exist.
- [ ] Legacy workflow drains before bespoke scheduler removal.
- [ ] No stale authority or blind ambiguous replay is possible.
- [ ] Full project checks and independent review pass.

Initiative completion requires all selected runtime workflows to use the shared engine or have an explicit documented reason they are ineligible.


## Umbrella authority and dependency

Task 47003 is the sole authority for shared-engine ownership, sequencing, migration gates, and release criteria. This task preserves its historical design context and narrower acceptance detail, but any conflicting creation-first, wake-only, bespoke-scheduler, or rollout direction is superseded. Implementations SHALL follow `specs/durable-workflows/requirements.md` and ADR-010 through ADR-012.

This task remains blocked until engine-backed creation cutover and legacy drain complete.
