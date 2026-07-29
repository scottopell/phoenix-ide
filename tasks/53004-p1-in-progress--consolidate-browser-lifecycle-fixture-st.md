# Consolidate browser lifecycle fixture startup

Preserve every distinct browser ownership topology and lifecycle assertion while consolidating repeated browser startup and fixture cost. Profile the family before changes, prove equivalent lifecycle-fault detection, and keep this as a separate stacked PR after the merge-ready hard-delete ceiling slice.

## Evidence

Baseline: `c1a801d2c52c1b4ae1c1fec4569132e47bbd6ac3` (`origin/main` after PR #612 merged).
Final measured checkpoint: `20ae613bc`.

Profiler artifacts and fault logs are retained under `target/qa-evidence/53004/`:

- `direct-baseline/`: six exact baseline lifecycle tests executed from one immutable test binary.
- `direct-final-cleanup/`: final serialized fixture test from an immutable test binary.
- `baseline-launches.txt` / `final-cleanup-measured-launches.txt`: Chrome launch counts from a test-only exec wrapper.
- `before-rust-clean-c1a801d2c...` / `after-rust-63b22473f...`: warmed Rust lane profiles.
- `fault-preservation.log`, `fault-isolation.log`, `fault-restricted-owner-teardown.log`, `fault-no-inheritor-teardown.log`, `fault-different-scope-teardown.log`: deliberate production ownership faults rejected at their named retained scenario assertions.
- `fault-cleanup.log`: preservation fault proves scenario panic is propagated after fixture cleanup.

| Scope | Before | After | Absolute saving | Saving |
|---|---:|---:|---:|---:|
| Chrome launches | 8 | 4 | 4 launches | 50.00% |
| Targeted lifecycle CPU | 9,562.71 ms | 4,979.19 ms | 4,583.52 ms | 47.93% |
| Targeted lifecycle wall | 4,030.23 ms | 3,090.28 ms | 939.95 ms | 23.32% |
| Warmed Rust lane CPU | 481,872.57 ms | 481,737.48 ms | 135.10 ms | 0.03% |

Four launches are the topology-preserving minimum: two simultaneous work-scope sessions prove scope isolation, and two simultaneous restricted-actor sessions prove actor-private isolation and selective teardown. The full-lane difference is noise-floor and no broad speedup is claimed.

Host pressure initially prevented a clean lane baseline: 232 durable tmux processes and roughly 2,850 total processes produced an unrelated `fork failed` in a tmux test. No user/durable processes were killed; `./dev.py reap --dry-run` found no owned orphan servers. Task 45004 was already complete. Clean direct-binary measurements and later successful warmed lane runs were used instead.
