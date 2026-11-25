# Fix 4 flaky `phoenix-tools` tests on the slow-zombie-reaping container host

## Root cause (confirmed by code reading, not yet by a live test run — no write/build access in Explore mode)

All 4 failures share one mechanism: on this container host (`sopell-worker-one`),
when a process's real parent has exited, the process is reparented to a `PID 1`
(or subreaper) that does not promptly call `waitpid` on it. The dead process
sits as a zombie (`State: Z` in `/proc/<pid>/stat`, `ps` state `Z`/`X`) for an
unbounded time. Two different naive liveness checks in **test code** — not
production code — wrongly treat that lingering zombie as "still running" and
spin until their deadline:

- `libc::kill(pid_or_pgid, 0)` returns `0` (success) for a zombie — the kernel
  still has a process-table entry, it just has no resources left. It only
  returns `ESRCH` once something actually reaps it.
- `phoenix_core::process_identity::current_process_identity` /
  `process_identity_matches` read `/proc/<pid>/stat`'s `starttime` field, which
  is still present and unchanged for a zombie. So `process_identity_matches`
  returns `true` for a zombie exactly as it does for a live process.

This is a **known, already-handled pattern elsewhere in this codebase**, not a
new discovery:

- `crates/phoenix-tools/src/bash/registry.rs::linux_retirement_target_has_live_member`
  (+ `parse_linux_proc_stat`) is the production Linux check for "is this
  process group actually still running". It enumerates `/proc`, matches pgid,
  and explicitly excludes `state == b'Z' || state == b'X'` members. This is
  correct and should not change.
- `specs/browser-cli/executive.md` already documents, for a different test
  (`dead_chrome_cleanup_failure_remains_visible_until_explicit_close`), that
  "an unreaped zombie is a supported observation, not a measured cause" and
  that the test must accept "birth-identity disappearance **or** a
  kernel-exited/zombie observation." `tests/browser_cli.rs::observe_process_exited_or_retained_zombie`
  implements exactly that tolerance (checks `ps -o stat=` for `Z`/`X` as an
  accepted terminal state).

The 4 failing tests are the ones that **don't** use that established
tolerance:

1. `bash::operations::tests::dropping_unregistered_spawn_kills_the_process_group`
   spawns `(echo ready; exec sleep 30) & wait` — the process group's `sleep`
   member is a *grandchild* of the test (forked by the shell via `&`), so it's
   never a tracked child of the Rust process. After `drop(spawned)` sends
   `SIGKILL` to `-pgid`, the shell (a direct, tokio-tracked child) is reaped
   promptly, but the `sleep` grandchild is reparented on the shell's death and
   becomes a zombie under this host's slow-reaping ancestor. The test's own
   poll loop (`libc::kill(-pgid, 0)` until `ESRCH`) can never observe `ESRCH`
   while that zombie lingers, so it times out at the 5s deadline.

2. `partial_clients_do_not_block_close_signals_or_idle_cleanup`,
   `sigterm_during_chrome_startup_cleans_up_the_published_browser`, and
   `active_work_outlives_idle_budget_then_session_expires` all call
   `await_process_exit(daemon)` (and the sigterm test also calls it on
   `chrome`). The browser-cli daemon is deliberately detached: see
   `crates/phoenix-tools/src/bin/phoenix-browser/cli.rs::connect_or_start` —
   it spawns the daemon with `std::process::Command::new(...).spawn()` and then
   does `drop(daemon.0.take())` once the daemon is ready, intentionally never
   calling `wait()` on it so the short-lived CLI process can exit without
   blocking. Once the CLI process exits, the long-running daemon is reparented
   to the slow-reaping ancestor. That's correct, intended production behavior
   — the bug is that `await_process_exit` (`crates/phoenix-tools/tests/browser_cli.rs`,
   `fn await_process_exit`) requires `!process_identity_matches(process)`,
   which never becomes true for a lingering zombie, so all 3 tests time out at
   the 20s deadline (reported at `browser_cli.rs:769`, the `wait_for_observation`
   call site inside `await_process_exit`).

None of this is a genuine resource leak: cleanup (profile dir removal, socket
removal) happens before/as part of the owning process's own exit, and a
zombie holds no memory/CPU/fds. The tests are over-strict about *how* exit is
observed on a host where Phoenix has no control over orphan reaping.

## Fix plan (test-only; no production behavior change)

This mirrors the existing, already-accepted pattern — do not invent a third
liveness-checking convention and do not change `phoenix_core::process_identity`
(it's a shared primitive with its own carefully-scoped zombie handling at other
call sites, e.g. `phoenix-ide/src/runtime/close_retirement.rs`'s
thread-group-aware zombie exclusion for *write authority* — broadening the
shared primitive's zombie semantics risks that unrelated, more sensitive call
site).

**1. `bash::operations::tests::dropping_unregistered_spawn_kills_the_process_group`**

Extract the Linux-specific zombie-aware group-liveness body of
`linux_retirement_target_has_live_member` in `bash/registry.rs` into a
smaller, reusable `pub(crate)` helper keyed only on `pgid` (e.g.
`linux_process_group_has_live_member(pgid: i32) -> std::io::Result<bool>`),
and have `linux_retirement_target_has_live_member` call it with
`target.pgid`. Update the test's poll loop
(`operations.rs::dropping_unregistered_spawn_kills_the_process_group`) to use
this helper on `#[cfg(target_os = "linux")]` (loop until `Ok(false)`), keeping
the existing raw `kill(-pgid, 0)`/`ESRCH` loop on
`#[cfg(all(unix, not(target_os = "linux")))]` — same split production already
uses in `registry.rs::verify_exact_absence`. Keep the 5s deadline and the
`!scratch.exists()` assertion unchanged.

**2. The 3 browser-cli tests**

`tests/browser_cli.rs` already has `observe_process_exited_or_retained_zombie`
which correctly tolerates the zombie/dead `ps` state. Make `await_process_exit`
use the same tolerance instead of a strict `!process_identity_matches` check —
simplest is to have `await_process_exit` delegate to
`observe_process_exited_or_retained_zombie` (or factor out one shared
predicate both call), so all `await_process_exit` call sites
(`partial_clients_do_not_block_close_signals_or_idle_cleanup`,
`sigterm_during_chrome_startup_cleans_up_the_published_browser` ×2,
`active_work_outlives_idle_budget_then_session_expires`, and the existing one
at `browser_cli.rs:2431`) get the host-independent tolerance. Check whether the
per-poll `eprintln!` diagnostic in `observe_process_exited_or_retained_zombie`
is still proportionate once it's the shared implementation (e.g. only log when
`cfg!(test)` noise matters, or keep it — it already runs at 20ms/poll for up to
20s elsewhere without complaint).

**3. Specs**

`specs/browser-cli/executive.md` already describes this zombie-tolerance
pattern for `dead_chrome_cleanup_failure_remains_visible_until_explicit_close`.
If step 2 merges/renames the helper, check whether that paragraph still reads
accurately (it refers to the pattern conceptually, not by function name — likely
fine, but verify). No other spec changes are expected; this is a test-liveness
fix, not a behavior or requirement change.

## Verification

```bash
PATH="$HOME/.local/bin:$HOME/.cargo/bin:$PATH"
# + PHOENIX_CHROME_EXECUTABLE set to the Playwright Chromium under
#   /home/bits/.cache/ms-playwright/, and PHOENIX_BASH_LAUNCH_TEST_EXE set to
#   the built phoenix_ide binary, per the reproducer below.
cargo nextest run -E 'test(=bash::operations::tests::dropping_unregistered_spawn_kills_the_process_group) | test(=active_work_outlives_idle_budget_then_session_expires) | test(=partial_clients_do_not_block_close_signals_or_idle_cleanup) | test(=sigterm_during_chrome_startup_cleans_up_the_published_browser)' --no-fail-fast
```

Run it a few times in a row (not just once) — these are exactly the kind of
host-timing-dependent tests that can pass by luck. Then run the full
`phoenix-tools` test suite (`cargo test -p phoenix-tools` /
`cargo nextest run -p phoenix-tools`) to make sure the extraction/refactor
didn't regress `registry.rs`'s other retirement tests, then `./dev.py check`
until green.

## Boundaries

- Changes stay inside `crates/phoenix-tools` (and `specs/browser-cli/` if the
  wording needs a touch-up). No change to `phoenix-core::process_identity` or
  other crates.
- Do not touch `/home/bits/dev/phoenix-ide` (the running installation) or
  unrelated repos.
- No merge, deploy, release, or remote push. Leave clean, verified local
  commits on the task branch for review.
