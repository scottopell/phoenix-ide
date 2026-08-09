# Parallel Work subagent lifecycle and result delivery

## Product journey

A parent Work conversation can run multiple Work child agents concurrently without allowing overlapping writes to silently corrupt or replace sibling work. The architecture must provide an enforceable ownership, isolation, serialization, or conflict-detection boundary; LLM-authored assignment discipline alone is not a correctness boundary. Whether children share the parent writable environment directly or use isolated write targets is an architecture decision, not a precondition of the feature.

This remains a desired P1 product feature. It is blocked until the ProductConversation Close/subagent settlement integration boundary is stable enough to build against. It must not delay, widen, or destabilize the P0 ProductConversation Close, settlement, WorkScope-retirement, History, or deletion stack.

## Required architecture decisions

- **ChildMaterializationOwner:** define how fresh creation and reconstruction share one owner and one typed completion while preserving exactly-once initial task bootstrap.
- **InstalledChildCancelRoute:** make an installed child structurally and immediately cancellable without waiting behind unrelated spawn materialization.
- **ParentResultSink:** define who durably retains `{child_id, parent_id, outcome}` until exact parent acceptance, refreshes stale parent routes, retries delivery across Phoenix process restart, and cleans up. REQ-SA-009 requires restart-durable terminal evidence and delivery; a process-local pending marker is insufficient.
- **Write-conflict boundary:** choose and enforce structural write ownership, child isolation plus reconciliation, serialization, or conflict detection that covers every writable tool path. Prompt instructions and disjoint task descriptions are guidance only.
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
- Overlapping sibling write attempts cannot silently overwrite or interleave changes, including writes performed through bash or Git rather than patch.
- A child terminal outcome persisted before Phoenix restart is reconstructed and delivered after restart when the parent has not yet accepted it.

## Timing

ProductConversation defines the stable lifecycle/settlement contract; parallel Work integrates with it, never the reverse. Work may begin during ProductConversation development if its owner confirms the typed Close/subagent settlement seam is stable, otherwise after the P0 stack lands.

## Evidence source

PR #635 and its exact-head review preserve the abandoned implementation, race analysis, and tests as architecture evidence.
