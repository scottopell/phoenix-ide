# ADR-014: Workflow transitions use CAS and every claimed effect uses leased authority

- **Status:** Accepted
- **Date:** 2026-07-11
- **Affects:** REQ-DWF-004, REQ-DWF-005, REQ-DWF-006, REQ-DWF-007, REQ-DWF-008, REQ-DWF-009, REQ-DWF-010, REQ-DWF-011, REQ-DWF-012, REQ-DWF-018

## Context

A workflow reducer needs one serialized history, but independent external effects
need concurrency. Workflow-version compare-and-swap can serialize reducer commits;
it cannot prove that a worker still owns an external step after a lease expiry,
takeover, or cancellation. Conversely, an effect lease cannot serialize two
competing product transitions or ensure that a transition and its complete DAG
commit together.

External systems also admit acknowledgement loss. Treating lease expiry as effect
failure and blindly replaying would duplicate actions whose outcome is unknown.

## Options considered

1. **Use workflow CAS for every operation.** This serializes all work and still
   lacks worker/token lease fencing across the external side-effect window.
2. **Use one workflow-level lease.** This fences a worker but prevents safe
   parallel effects and couples unrelated long operations.
3. **Separate transition and effect authority.** Serialize reducer plans with
   expected workflow-version CAS; execute every claimed effect under generation,
   token, worker, and finite lease authority; require a declared ambiguity policy.

## Decision

Adopt option 3. A reducer transition atomically advances one workflow version and
commits its snapshot, transition record, complete effect DAG, barriers, and any
cancellation compensation. Independent eligible effects may then be claimed in
parallel. Every claimed inspection, mutation, observation, retry, and receipt
commit requires matching live workflow generation, effect identity, claim token,
worker, and lease.

Every effect family declares exactly one ambiguity policy: observable
reconciliation, externally enforced idempotency, safe repeatability, or manual
resolution. Lease expiry enables takeover but never proves external failure.
Destructive profile effects also acquire their physical resource lock.
Barrier membership is a normalized typed contract: a receipt satisfies a member
only when it belongs to the current generation, the same effect, and the receipt
family declared by the profile. Compensation has separate membership and cannot
stand in for required forward work. Manual resolution persists normalized permitted
choice rows with kind and codec; resolution references the accepted row rather than
copying an untyped choice payload.

## Consequences

- **Positive:** Competing reducer events have one winner while independent effects
  retain concurrency.
- **Positive:** Cancellation revokes stale workers immediately through generation
  change, and stale results cannot commit.
- **Positive:** Ambiguous outcomes have an explicit recovery contract rather than
  a false exactly-once promise.
- **Negative:** Every executor path must carry and check a richer authority token;
  normal authority loss becomes an expected typed outcome.
- **Negative:** Reconciliation adapters and manual-resolution surfaces are required
  for effects that cannot safely repeat.
- **Neutral:** In-memory kicks and locks may improve latency, but durable deadlines
  and database authority remain the correctness boundary.

## References

- Related ADRs: ADR-007, ADR-013, ADR-015
- Feature spec: `specs/durable-workflows/requirements.md`
