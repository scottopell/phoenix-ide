# ADR-019: Durable-workflow core matches one scheduler authority and durable acknowledgement

- **Status:** Accepted
- **Date:** 2026-07-18
- **Affects:** REQ-DWF-002, REQ-DWF-006, REQ-DWF-014, REQ-DWF-017, REQ-DWF-029 through REQ-DWF-042, REQ-DWF-CHAT-001 through REQ-DWF-CHAT-011, REQ-DWF-WAKE-002 through REQ-DWF-WAKE-005, REQ-DWF-CREATE-001 through REQ-DWF-CREATE-004

## Context

The earlier durable-workflow ADR chain establishes important invariants: the
product reducer remains the sole semantic authority; execution truth belongs in a
normalized core; evidence, receipts, and runtime acceptance are distinct durable
facts; and client-visible acceptance belongs inside the durable boundary when it
can be replayed. The same chain also assumes two properties that no longer match
the chosen Phoenix topology and upgrade promise.

First, it treats every claimed external step as universally leased work. That is
too broad for Phoenix's one-server topology because lease expiry cannot prove that
a remote mutation stopped, and some steps become safe to reclaim only when their
capability class explicitly permits it.

Second, it treats protocol selection, retained old executors, shadow authority,
rollback selectors, and exact zero-authority drain as permanent core semantics.
That is too broad for Phoenix's upgrade contract, which is to migrate typed
persisted intent and evidence, restart under current semantics, and surface any
incompatible active work explicitly rather than keep an indefinite multi-version
deployment platform inside the steady-state engine.

The durable-workflow contract still needs to cover wake, creation, direct turns,
future coordinator loops, and long-running remote executors. Direct turns in
particular need stable resolved-target message identity, immutable prepared
payloads that resolve file and skill inputs before acceptance, typed committed
and replay outcomes, target-local runtime arbitration, exact runtime
reconciliation, and independent per-target fan-out without letting secondary
consumers redefine acceptance.
The question is not whether Phoenix keeps a shared durable-workflow engine. The
question is which facts the permanent engine must represent structurally.

## Options considered

1. **Keep ADR-013 through ADR-016 unchanged.** This preserves the earlier general
   model, but it permanently represents selector, shadow, rollback, drain, and
   universal-lease state that Phoenix does not need in its chosen topology.
2. **Retreat to bespoke per-feature schedulers.** This removes generic machinery,
   but it reintroduces duplicated crash-boundary, ambiguity, delivery, and
   cancellation logic across wake, creation, direct turns, and future scheduled
   loops.
3. **Keep the shared engine, but narrow the permanent core to one scheduler
   authority, durable acknowledgement, universal attempt fencing, optional
   reclaimable leases, canonical delivery, typed profile migration, submit-then-
   observe remote work, and explicit `CoalesceLatest` scheduling.** Migration-local
   comparison or drain tooling remains allowed when justified, but it is not part
   of the steady-state engine state model.

## Decision

Adopt option 3.

Phoenix keeps the shared durable-workflow engine and the earlier reducer-owned
semantic boundary. The permanent core is narrowed to the facts every admitted
profile actually needs in the chosen topology:

- one scheduler authority per SQLite database;
- durable acknowledgement as the adoption boundary;
- universal workflow version, generation, process-incarnation, and attempt
  fencing;
- leases only for reclaimable phases, with expiry meaning loss of local
  authority rather than proof that external execution stopped;
- structural execution capability classes that govern retry, takeover, and
  ambiguity handling;
- one canonical durable delivery lifecycle plus optional runtime-acceptance
  bookkeeping where the profile needs a later runtime start;
- direct-turn identities bound to resolved targets with immutable prepared
  payloads, typed committed and replay outcomes, target-local runtime
  arbitration, exact-ID runtime reconciliation, and independent per-target
  fan-out plus separate additional-consumer cursors;
- typed profile kind/version plus migrations or explicit incompatible-work
  preservation;
- durable submit, durable handle receipt when available, and reclaimable observe
  for long-running remote work;
- `CoalesceLatest` as the first explicit recurring schedule policy.

Permanent selector, executor-retention, shadow-authority, rollback-selector, and
exact-drain semantics are removed from the steady-state core contract. A specific
migration may still use temporary profile-local comparison, migration-local
drain inventory, or other risk-reduction tooling, but those mechanisms are not
engine invariants and do not create a second steady-state semantic authority.

This ADR supersedes the parts of ADR-013 through ADR-016 that require permanent
multi-version rollout machinery or universal leased effect authority. It does not
revoke their still-valid core conclusions about reducer semantic authority,
normalized execution truth, distinction between evidence and acceptance, or the
need for stable acceptance keys at replayable boundaries.

## Consequences

- **Positive:** The permanent engine state space matches Phoenix's actual topology
  and upgrade contract more closely, reducing invalid combinations the
  implementation must keep synchronized.
- **Positive:** The shared engine remains justified for wake, creation, direct
  turns, remote executors, and coordinator loops, so crash-boundary correctness
  is still centralized rather than reimplemented per feature.
- **Positive:** External ambiguity handling becomes more truthful because retry or
  takeover permission is structural in the capability class, not inferred from a
  generic expired lease.
- **Negative:** Earlier implementation and spec work built around selector,
  shadow, rollback, drain, and universal lease concepts must be reshaped rather
  than incrementally extended.
- **Negative:** Migration safety now depends on strong typed migration testing and
  explicit incompatible-work handling, because the engine no longer promises
  indefinite execution of every historical protocol version.
- **Neutral:** A high-risk migration may still temporarily introduce comparison or
  drain tooling, but that tooling is local to the migration rather than a
  permanent engine concept.

## References

- Supersedes parts of: ADR-013, ADR-014, ADR-015, ADR-016
- Related ADRs: ADR-007, ADR-011, ADR-012
- Feature spec: `specs/durable-workflows/requirements.md`
- Review document: `docs/durable-workflow-grand-vision-review.md`
