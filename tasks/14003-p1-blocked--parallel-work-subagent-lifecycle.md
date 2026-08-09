# Parallel Work subagent lifecycle and result delivery

## Product journey

A parent Work conversation can run multiple Work child agents concurrently in the parent's writable environment. Work/Branch children share the parent worktree; Direct children share the parent cwd. Sibling writes are not locked, Phoenix performs no child merge, and assignments should be disjoint.

This remains a desired P1 product feature. It is blocked until the ProductConversation Close/subagent settlement integration boundary is stable enough to build against. It must not delay, widen, or destabilize the P0 ProductConversation Close, settlement, WorkScope-retirement, History, or deletion stack.

## Required architecture decisions

- **ChildMaterializationOwner:** define how fresh creation and reconstruction share one owner and one typed completion while preserving exactly-once initial task bootstrap.
- **InstalledChildCancelRoute:** make an installed child structurally and immediately cancellable without waiting behind unrelated spawn materialization.
- **ParentResultSink:** define who retains `{child_id, parent_id, outcome}` until exact parent acceptance, refreshes stale parent routes, retries delivery, and cleans up.
- Decide explicitly whether terminal child results survive Phoenix process restart. If yes, design a durable outbox rather than hiding durability inside an in-memory enum.
- Define how ProductConversation Close consumes one typed subagent settlement operation without reproducing child lifecycle or fan-in authority.

## Authority constraints

- The child state machine remains authoritative for child execution.
- The parent state machine remains authoritative for pending children and fan-in.
- The runtime map remains routing only.
- WorkScope remains resource ownership authority.
- ProductConversation owns Close orchestration and delivery policy.
- No optional constructor wiring or parallel admission representations.
- No `select!` or channel-consumer ordering may determine semantic admission.

## Required evidence

- Cancellation before dequeue suppresses materialization exactly once.
- Cancellation during materialization is delivered exactly once after installation.
- Installed-child cancellation bypasses unrelated slow materialization.
- Fresh construction and reconstruction converge on one runtime.
- Initial task injection occurs exactly once.
- Terminal outcome remains available until exact parent acceptance.
- Parent route replacement/reconstruction cannot lose the result.
- Process-restart behavior is explicit and tested.
- ProductConversation Close settles parallel children through one typed operation.
- Sequential Work-child behavior remains valid.

## Timing

ProductConversation defines the stable lifecycle/settlement contract; parallel Work integrates with it, never the reverse. Work may begin during ProductConversation development if its owner confirms the typed Close/subagent settlement seam is stable, otherwise after the P0 stack lands.

## Evidence source

PR #635 and its exact-head review preserve the abandoned implementation, race analysis, and tests as architecture evidence.
