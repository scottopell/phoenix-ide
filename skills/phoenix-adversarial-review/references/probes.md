# Adversarial probe catalog

Choose probes that can change the verdict. Do not mechanically execute every row.

| Changed seam | Questions that can falsify it |
|---|---|
| Types / constructors | Can invalid values enter through serde, DB rows, tests, defaults, cloning, or a public field? Is validation bypassed on reconstruction? |
| Producer → consumer | Are field names, discriminants, marker literals, units, ordering, null/empty semantics, versions, and truncation identical at both sites? |
| Persistence / migration | Does old data migrate losslessly? Are constraints equivalent under SQLite semantics, including `NULL`? Is every addressable child normalized? Can partial migration or rollback strand mixed generations? |
| Transaction / effect | Is durable authority committed before an external effect or acknowledgment? On error, which observations or writes must still persist? Can two contenders both pass a preflight? |
| State machine | Is every transition legal from all reachable states? Does every effect return through the reducer? Can cancellation, stale results, duplicates, or retries skip cleanup or terminalization? |
| SSE / cache / UI reducer | Can reconnect, prepend, pagination, rename, archive, or out-of-order delivery duplicate, resurrect, drop, or mis-key state? Does the producer publish every committed mutation? |
| Async / concurrency | What owns outstanding work? What proves completion? Can cancellation win before registration or after the effect? Are race losers typed outcomes rather than raw errors? |
| Timer / retry / wake | Is time the behavior or merely a surrogate signal? Are deadlines based on event time or observation time? Is retry safe for the capability and idempotency class? |
| Restart / respawn | What remains after process memory disappears? Can durable state reconstruct external resources? Does generation rotate whenever an incarnation changes, including in-place respawn? |
| Resource cleanup | What survives failure between creation, registration, cancellation, and cleanup? Does deletion run cleanup before erasing ownership rows? Is cleanup replayable? |
| Provider / protocol | Are streamed and non-streamed terminal/error cases classified consistently? Are token boundaries arbitrary? Are unsupported capabilities logged rather than silently dropped? |
| Security / trust | Is recalled, remote, browser, or tool-provided text treated as untrusted data? Is authorization checked at the effect boundary, not only in UI? Can server-local paths or actions leak across hosts? |
| Git / worktree | Is the target ref checked out elsewhere? Does fetch stay separate from local ref movement? Are base/head SHAs fixed before review or mutation? |
| Tests | Does the test observe the owning postcondition? Would a no-op implementation pass? Are error details, old-row behavior, restart, duplicate, and negative paths covered? |
| Complexity / YAGNI | Which named invariant or capability pays for each abstraction, selector, cache, compatibility path, or dependency? Can one authority or direct path replace parallel machinery? |

## Corpus-informed concentration areas

A bounded review-history sample showed repeated findings clustering around these moves. Treat this as search-order guidance, not frequency or quality claims:

1. **Preserve facts across transitions.** Trace values, evidence, compensation, and ownership through success, failure, cancellation, retry, terminalization, and projection.
2. **Challenge ordering and atomicity.** Look for work admitted before registration, effects performed before durable ownership, cleanup after authority deletion, stale race winners, and ambiguous commits.
3. **Bind identity to scope and generation.** Verify that replay, respawn, migration, and selection carry the exact repository, worktree, conversation, attempt, generation, or incarnation—not a nearby surrogate.
4. **Test recovery from durable facts alone.** Remove process memory, then test restart, old rows, partial migrations, surviving resources, and retries.
5. **Inspect rejected and empty states.** Exercise `NULL`, absent, empty, duplicate, stale, retired, terminal, overflow, and bounded-capacity cases at the real authority boundary.
6. **Compare projections with their source.** Follow authoritative state into database views, SSE, caches, UI selectors, summaries, and specs; verify filtering and naming stay equivalent.
7. **Keep cleanup evidence until cleanup succeeds.** Failure compensation must remain discoverable and replayable rather than being erased by the failed transition it is meant to repair.

These themes may originate from Codex-only corpus evidence. They become claims about local-review blind spots only in exact-HEAD or lineage-confirmed near-match comparisons.

## Cross-boundary trace template

For each risky value, write a one-line trace:

```text
origin/type → serialization or call → durable owner → replay/recovery → consumer → visible effect
```

At each arrow ask:

- Can the value be omitted, defaulted, duplicated, renamed, truncated, or reordered?
- Is ownership transferred atomically?
- What happens if the process stops immediately before and after the arrow?
- Which test would fail if this arrow were removed?

## Counterexample generation

Prefer the smallest counterexample that crosses the changed seam:

- empty vs absent vs `NULL`;
- first, last, duplicate, stale, or out-of-order item;
- two concurrent contenders;
- cancellation immediately before/after registration;
- restart with a surviving external process;
- in-place respawn with reused resource identity;
- producer literal changed while parser stays old;
- old persisted row plus new binary;
- error carrying useful observations;
- clean result with no findings, to test false-positive resistance.
