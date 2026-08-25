# ProductConversation Close settlement and WorkScope retirement orchestration

## Boundary
Own the live Close lifecycle after the dormant authority/evidence foundation lands. This task is the sole owner of settlement authority release, fresh workspace inspection, resource-creation admission, exact retirement-inventory capture, destructive WorkScope retirement, evidence recording, needs-repair retry, and crash recovery through the retirement boundary.

## Acceptance criteria
- [ ] Settlement cannot advance until all current aggregate authorities on the exact latest row are released.
- [ ] Retirement authorization recomputes authoritative Git/workspace loss evidence at confirmation/inventory time and invalidates stale approval.
- [ ] Resource creation for every targeted WorkScope is fenced from inventory capture through retirement settlement, with deterministic multi-scope acquisition and restart recovery.
- [ ] The exact persisted inventory/evidence APIs from the foundation are composed transactionally; no parallel lifecycle authority is introduced.
- [ ] Cancellation, crash, retry, and concurrent-creation schedules have deterministic regressions.

## Verified implementation handoff

The Close persistence foundation is present, but has no production coordinator or callers. A database-only settlement transition was prototyped and reverted after immutable adversarial review: it missed accepted `durable_turns.owns_conversation`, trusted caller-supplied snapshot strings as fresh inspection evidence, and left direct-turn admission unfenced.

Next implementation must be runtime-owned and structural:

1. Add one aggregate Close coordinator, recovering `list_pending_close_obligations()` at runtime startup and owning explicit stop-work cancellation through settlement.
2. Establish a durable aggregate admission fence before settlement; direct-turn acceptance, wake delivery, tool/sub-agent work, and each WorkScope resource creator must acquire it before admitting work. Hold ordered WorkScope fences from inventory capture through evidence-backed retirement/retry.
3. Have the coordinator recompute Git/workspace inspection at confirmation and inventory time; pass the resulting authoritative typed evidence to the existing foundation APIs rather than accepting a client snapshot.
4. Retire one sealed inventory resource at a time and record proof/residual immediately. Leave final History/outcome/FTS publication to task 92032.

The specific rejected interleavings are documented by the immutable review in this conversation: idle projection with a live direct-turn owner; direct-turn acceptance after settlement begins; and replay of a stale displayed generation/fingerprint after workspace mutation.
