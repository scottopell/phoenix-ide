# Reduce Git HEAD observation stress cost without weakening consistency coverage

Preserve one bounded concurrent real-Git integration case while extracting the six-attempt HEAD snapshot algorithm behind a deterministic seam. Cover branch/ref changes, detached and unborn HEAD, transient failures, retry exhaustion, and consistency decisions. Prove the deterministic suite catches the same deliberate consistency fault as the current stress test, and retain before/after CPU evidence from the check profiler.

## Evidence

Baseline commit: `fd80bd35f478eefad3e68bd163bca921dbe84489` (merged PR #597).
Implementation checkpoint: `b50fa4fa53ade2a872f1d1ed7217f9771087f505`.

Profiler artifacts are retained under `target/qa-evidence/53002/`:

- `before-exact-fd80bd35f...`: exact stress-test command baseline.
- `after-final-77bebf2f2...`: same exact command after review-strengthening the bounded integration case.
- `before-rust-fd80bd35f...`: warmed Rust check baseline.
- `after-rust-warm-b50fa4fa5...`: warmed Rust check after extraction.
- `fault-existing-stress.log`: existing concurrent stress test catches `feature + main OID` mismatch.
- `fault-deterministic.log`: deterministic branch-change test catches the same `feature + main-oid` mismatch.

Measured results (the 99.05% result is scoped to the targeted test command, not the whole Rust suite):

| Scope | Before CPU | After CPU | Absolute saving | Saving |
|---|---:|---:|---:|---:|
| Exact test command | 122,724.72 ms | 1,165.22 ms | 121,559.50 ms | 99.05% |
| Warmed full Rust check command | 501,591.40 ms | 501,094.14 ms | 497.27 ms | 0.10% |

Exact-command wall time fell from 36,158.84 ms to 1,205.42 ms: 34,953.42 ms / 96.67% saved. The full Rust aggregate contains thousands of unrelated tests and is load-sensitive, so its 0.10% change is not evidence of a suite-wide speedup. A final warmed lane measurement is recorded after the review-strengthened integration workload.
