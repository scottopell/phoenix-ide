# ProductConversation History finalization and deletion

## Boundary
Own terminal ProductConversation Close finalization after retirement orchestration succeeds. This task is the sole owner of the durable Close outcome message, aggregate History transition, typed completed outcome commit, FTS projection, exact-attempt replay after commit/result loss, History projection, and lifecycle-aware archive/delete cleanup.

## Acceptance criteria
- [ ] Archived finalization is one exact-attempt transaction across outcome message, FTS projection, captured-member History state, and completed archived outcome.
- [ ] Replaying the exact committed finalization converges without duplicate messages or divergent terminal state.
- [ ] Cancelled completion remains distinguishable from archived completion through the typed foundation model.
- [ ] History listing projects ProductConversation aggregates without mirroring or parallel lifecycle authority.
- [ ] Archive/delete cleanup uses durable, crash-recoverable authority and cannot race active Close or durable turns.

Blocked on Close orchestration and the dormant Close authority/evidence foundation.
