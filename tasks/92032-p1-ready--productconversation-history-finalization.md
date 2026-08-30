# ProductConversation minimal History finalization

## Boundary

Own the minimal terminal ProductConversation finalizer after Close retirement succeeds: one idempotent atomic successful Close outcome and aggregate History lifecycle transition, plus read-only History list/view projection.

## Acceptance criteria

- [ ] One exact-attempt transaction records the successful Close outcome and moves the ordinary ProductConversation to History.
- [ ] Replaying that exact finalization converges without a duplicate outcome or divergent lifecycle.
- [ ] History is read-only and projects ProductConversation aggregates without a second lifecycle authority.
- [ ] The finalizer persists only the completion obligation required by this minimal transition.
- [ ] This milestone excludes FTS, permanent deletion, restore, retention, bulk lifecycle operations, and lifecycle-aware cleanup.

Ready after Close orchestration and its durable completion boundary are available.
