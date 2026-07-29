# Consolidate browser lifecycle fixture startup

Preserve every distinct browser ownership topology and lifecycle assertion while consolidating repeated browser startup and fixture cost. Profile the family before changes, prove equivalent lifecycle-fault detection, and keep this as a separate stacked PR after the merge-ready hard-delete ceiling slice.

## Evidence

Baseline: `c1a801d2c52c1b4ae1c1fec4569132e47bbd6ac3` (`origin/main` after PR #612 merged).
Final measured checkpoint: `20ae613bc`.

Profiler artifacts and fault logs are retained under `target/qa-evidence/53004/`:

- `direct-baseline/`: six exact baseline lifecycle tests executed from one immutable test binary.
- `direct-codex-final/`: final serialized fixture test from an immutable test binary after restricted teardown review fixes.
- `baseline-launches.txt` / `codex-final-measured-launches.txt`: Chrome launch counts from a test-only exec wrapper.
- `before-rust-clean-c1a801d2c...` / `after-rust-63b22473f...`: warmed Rust lane profiles.
- `fault-preservation.log`, `fault-isolation.log`, `fault-restricted-selective-noop.log`, `fault-restricted-full-scope-first-only.log`, `fault-no-inheritor-teardown.log`, `fault-different-scope-teardown.log`: deliberate production ownership faults rejected at their named retained scenario assertions.
- `fault-cleanup.log`: preservation fault proves scenario panic is propagated after fixture cleanup.

| Scope | Before | After | Absolute saving | Saving |
|---|---:|---:|---:|---:|
| Chrome launches | 8 | 5 | 3 launches | 37.50% |
| Targeted lifecycle CPU | 9,562.71 ms | 6,190.75 ms | 3,371.96 ms | 35.26% |
| Targeted lifecycle wall | 4,030.23 ms | 3,677.84 ms | 352.39 ms | 8.74% |
| Warmed Rust lane CPU | 481,872.57 ms | 481,737.48 ms | 135.10 ms | 0.03% |

Five launches are required to preserve the full reviewed topology: two work-scope sessions prove scope isolation; two simultaneous restricted-actor sessions prove actor-private isolation and selective teardown; and the selectively removed owner is relaunched so full-scope teardown is tested again with two live actor-private sessions. The full-lane difference is noise-floor and no broad speedup is claimed.

Host pressure initially prevented a clean lane baseline: 232 durable tmux processes and roughly 2,850 total processes produced an unrelated `fork failed` in a tmux test. No user/durable processes were killed; `./dev.py reap --dry-run` found no owned orphan servers. Task 45004 was already complete. Clean direct-binary measurements and later successful warmed lane runs were used instead.
