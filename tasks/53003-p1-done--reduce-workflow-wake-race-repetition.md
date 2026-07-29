# Reduce workflow wake-race repetition without weakening ownership invariants

Measure the repeated wake race family, preserve real SQLite concurrency coverage, move repeated ownership permutations to deterministic seams where possible, and prove equivalent fault detection. Keep this as a separate measured slice stacked after task 53002 until PR #609 merges.

## Evidence

Baseline parent: `d7506c6878a6a8517b5231815a7d849e20b8de24` (stacked after merge-ready PR #609).
Final measured checkpoint: `a03344133`.

Profiler artifacts are retained under `target/qa-evidence/53003/`:

- `before-family-warm-d7506c687...`: five repeated race tests, 50 real SQLite races.
- `after-family-final-a03344133...`: six deterministic ordered winner scenarios plus five real concurrent SQLite races.
- `after-rust-b8a7b5536...`: profiled Rust lane after the matrix extraction.
- `fault-existing-owner-admission.log`: repeated transfer race rejects a transfer that incorrectly returns `OwnerMismatch` for the current owner.
- `fault-deterministic-owner-admission.log`: deterministic transfer-before-terminal test rejects the same fault because it requires `Transferred` and ownership migration.

Two candidate transfer mutants were rejected as invalid proof because the existing race did not catch them: swapping receipt-migration binds and suppressing the binding update. This clarified that the retained race's actual invariant is typed transfer admission plus coherent ownership, while existing ordered transfer tests prove successful migration.

| Scope | Before CPU | After CPU | Absolute saving | Saving |
|---|---:|---:|---:|---:|
| Targeted wake race family | 81,298.04 ms | 8,284.61 ms | 73,013.43 ms | 89.81% |
| Rust check command (load-sensitive) | 494,670.54 ms | 489,591.05 ms | 5,079.50 ms | 1.03% |

Targeted wall time fell from 21,587.31 ms to 4,222.14 ms: 17,365.17 ms / 80.44% saved. Production SQLite retry budgets are unchanged. Dedicated operation tests retain typed cancellation/expiry metadata coverage; the ordered matrix additionally asserts loser typing, terminal payload, one canonical delivery row, and one pending delivery.
