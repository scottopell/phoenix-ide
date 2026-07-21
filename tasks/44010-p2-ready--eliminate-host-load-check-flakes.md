# Eliminate host-load check flakes

## Problem

`./dev.py check` still produces load-sensitive false failures that disappear when rerun in isolation. During task 44009 validation, two distinct flakes appeared while the host load average was unusually high:

1. `phoenix-tools browser::tests::test_browser_profile_heap_snapshot_streaming_and_diff` timed out after 15 seconds waiting for local page navigation. Other browser tests completed, and the failure did not recur on the next check.
2. `phoenix-db workflow::wake::tests::transfer_vs_cancel_race_repeated_has_one_coherent_owner` failed with SQLite `database is locked` while the Rust lane reported load average `230.0/126.4/109.6`. The exact test passed immediately in isolation.

These failures obscure real regressions and force repeated full checks, which further increases contention. Diagnose the shared host-load sensitivity rather than increasing arbitrary sleeps or globally weakening assertions.

## Investigation

- Capture per-lane CPU, memory, process, Cargo-lock, browser, and SQLite contention around failing checks.
- Determine whether the check scheduler's concurrency cap accounts adequately for system load and I/O contention, not only CPU count and memory.
- For browser navigation, distinguish an overloaded local HTTP/CDP path from a genuinely hung browser and use readiness signals or bounded adaptive policy where appropriate.
- For SQLite race tests, identify why the test's retry/transaction boundary can leak `SQLITE_BUSY` under contention and make the concurrency invariant deterministic.
- Audit nearby tests for the same timeout and direct-unwrap-on-busy patterns so fixes address the bug class rather than these two names only.

## Acceptance criteria

- [ ] Both named tests remain reliable while run concurrently under a reproducible high-load harness.
- [ ] SQLite race tests handle expected transient lock contention without hiding invariant failures or extending retries without evidence.
- [ ] Browser integration tests use a readiness/timeout policy that survives expected check-lane contention and still fails promptly for real hangs.
- [ ] The check harness records enough load/contender evidence to classify any remaining timeout or lock failure from one run.
- [ ] Repeated full `./dev.py check` runs under induced load complete without either flake.
