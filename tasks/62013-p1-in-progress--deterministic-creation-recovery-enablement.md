# Deterministic creation-recovery enablement

Child of parent task 17003.

Commission B: add deterministic creation-recovery coverage for ProductConversation creation without changing production behavior.

## Scope

Primary scope is a deterministic integration test in `crates/phoenix-ide/src/runtime/creation_worker.rs` that proves explicit retry after a legitimate delivery failure reuses the same published identities and does not create a duplicate aggregate.

## Required outcome

Add a test that:

1. Drives ProductConversation creation far enough to reserve/persist the intended durable aggregate identities.
2. Reaches `delivery_failed` through a legitimate steering-queue admission failure during objective delivery, not through fabricated DB state, provider fault toggles, special endpoints, or arbitrary sleeps.
3. Invokes the existing explicit retry path.
4. Proves the eventual successful publish reuses the same `ProductConversation`, transcript row, and initial message identities established before the failed delivery attempt.
5. Proves no duplicate ProductConversation aggregate, transcript row, or duplicate objective aggregate is created across the failed delivery plus explicit retry sequence.

## Constraints

- Exact implementation base must remain `382a8aa5de1967f91839c74339a847a04e4778d3` when work begins.
- No production behavior changes.
- No provider fault toggle or test-only endpoint.
- No arbitrary sleeps; use deterministic barriers/hooks/owned test seams already present or add tightly scoped deterministic test instrumentation only if required.
- No invalid or fabricated durable DB state.
- Keep the test primarily in `crates/phoenix-ide/src/runtime/creation_worker.rs`.

## Route-test slice

A thin retry-route test may touch `crates/phoenix-ide/src/api/handlers.rs` only after checking active PR738 overlap. Because PR738 currently modifies `handlers.rs`, defer that slice unless the overlap is gone and separately authorized.

## Validation and review

- Run focused checks first, then full checks.
- Obtain `phoenix-adversarial-review` plus an independent review.
- Keep review/CI exact-head and immutable PR discipline.
- Resolve all review threads.
- Never merge or deploy from this commission.
