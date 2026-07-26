# Prevent tmux tests from leaking daemonized servers and exhausting PTYs

## Problem

`phoenix-tools` tmux tests start real tmux servers on sockets inside per-test `TempDir`s. The servers daemonize, are reparented to PID 1, and outlive the Rust test process unless each test reaches an explicit `tmux ... kill-server` call. Dropping `TempDir` removes the socket path but does not own or terminate the server.

Manual cleanup at the bottom of a test is not total: assertions, panics, nextest fail-fast cancellation, harness timeouts, or termination of the test process can skip it. Some historical tests had no cleanup at all. Repeated local runs accumulated 245 orphaned temporary servers and exhausted macOS's `kern.tty.ptmx_max=511`, causing `openpty(): out of pty devices` and tmux's secondary `fork failed: Device not configured` error.

Durable Phoenix tmux servers under `~/.phoenix-ide/tmux-sockets/` are intentional and must never be treated as test garbage.

## Proposed design

### 1. Make test server ownership explicit

Introduce one test-only owner type used by every test that may spawn tmux, rather than constructing `TempDir` and calling `kill-server` ad hoc.

The owner should:

- Allocate a unique test root and socket directory.
- Construct the `TmuxRegistry` for that directory.
- Track the exact socket paths/servers the test can create.
- Provide explicit async `shutdown()` that runs `tmux -S <socket> kill-server`, verifies the server is gone, and only then removes the directory.
- Implement a synchronous `Drop` backstop for Rust unwind/panic paths. Drop must not rely on a Tokio runtime; it should invoke bounded synchronous cleanup or terminate a directly owned supervisor/process group.
- Be impossible to construct with the durable production socket root.

Migrate all tmux tests to this owner and remove per-test best-effort cleanup blocks.

### 2. Own abrupt-process cleanup at the process boundary

RAII cannot run after SIGKILL, harness timeout escalation, or a forcibly terminated nextest process. Add a test-only process-lifetime containment mechanism:

- Preferred: launch each test tmux server beneath a test-owned supervisor whose lifetime is tied to the test harness through a parent-death mechanism available on the platform, or run the test tmux server in a process group that a harness-level cleanup guard can terminate.
- On macOS, where parent-death signals are unavailable, install a small watchdog/supervisor that retains a pipe from the test process and kills the exact tmux socket/server when the pipe reaches EOF. The tmux daemon must not inherit the watchdog pipe, otherwise EOF will never occur.
- Scope all cleanup to a run-specific test root/token. Never scan or kill by process name alone.
- If containment cannot be made reliable with tmux daemonization, add a dedicated test wrapper that records exact server identities in a run manifest and performs post-run cleanup from `./dev.py check`, including timeout/fail-fast paths.

RAII handles panic/unwind; the process-boundary mechanism handles abrupt test-process death. Both are required.

### 3. Detect regressions

Add verification that:

- A normally completed test leaves no server or PTY behind.
- A test that panics after spawning leaves no server after unwind.
- A child test harness killed after spawning is reclaimed by the watchdog/wrapper.
- Parallel tests only clean their own run-scoped servers.
- Durable sockets under `~/.phoenix-ide/tmux-sockets/` are structurally excluded.
- Cleanup failure is surfaced as a test/check failure rather than silently ignored.

Use before/after probes against exact run-owned socket identities. Do not assert global machine tmux counts because developers may have legitimate sessions.

## Acceptance criteria

- [x] Every real-server tmux test uses the centralized owner; no manual bottom-of-test `kill-server` cleanup remains.
- [x] Panic/unwind and Tokio task-cancellation cleanup are deterministic and tested.
- [x] Forced test-process and process-group termination cleanup are deterministic and tested on macOS.
- [x] Owner shutdown fails when exact run-owned servers remain live.
- [x] Cleanup is structurally limited to unique `/private/tmp/ptt-*` roots and cannot target durable Phoenix sockets.
- [x] Repeated and parallel tmux suites do not increase run-owned process, root, or PTY counts.
- [x] Existing tmux behavior tests pass in parallel.

## Verification evidence

A live host baseline found 169 PID-1-owned test tmux servers and 459 allocated PTYs. Narrow one-time cleanup of only `/var/folders/.../.tmp*` test servers returned the host to zero temporary orphans and 165 PTYs while preserving 130 durable Phoenix tmux servers.

The containment regressions exercise normal shutdown, panic unwind, Tokio task abort, direct runner `SIGKILL`, and runner-process-group `SIGKILL`. Each probes the exact owner socket and root after the lifecycle edge.

A full 68-test tmux suite, three concurrent suites, and five repeated suites were measured from a zero-owner baseline:

```text
baseline       ptys=257 test_servers=0 test_roots=0
after_single   ptys=257 test_servers=0 test_roots=0
after_parallel ptys=259 test_servers=0 test_roots=0
after_repeat_1 ptys=257 test_servers=0 test_roots=0
after_repeat_2 ptys=257 test_servers=0 test_roots=0
after_repeat_3 ptys=257 test_servers=0 test_roots=0
after_repeat_4 ptys=257 test_servers=0 test_roots=0
after_repeat_5 ptys=257 test_servers=0 test_roots=0
```

The two-PTY parallel delta was transient unrelated host activity; all owner roots and servers were zero immediately after the parallel phase, and PTYs returned to the exact baseline on every subsequent run.

After Codex identified and the implementation migrated the remaining real-server runtime test, the final matrix ran both the complete `phoenix-tools` tmux suite and `startup_restart_discovery_reuses_live_tmux_socket_without_registry_entry`:

```text
baseline       ptys=288 test_servers=0 test_roots=0
after_single   ptys=288 test_servers=0 test_roots=0
after_parallel ptys=288 test_servers=0 test_roots=0
after_repeat_1 ptys=288 test_servers=0 test_roots=0
after_repeat_2 ptys=288 test_servers=0 test_roots=0
after_repeat_3 ptys=288 test_servers=0 test_roots=0
after_repeat_4 ptys=288 test_servers=0 test_roots=0
after_repeat_5 ptys=288 test_servers=0 test_roots=0
```

The final decisive validation ran the actual repository check runner from a verified zero-orphan baseline, not a filtered test command:

```text
before: legacy_orphans=0 owner_servers=0 ptys=168
./dev.py check: all 18 applicable checks passed
after:  legacy_orphans=0 owner_servers=0 ptys=168
```

Serial full-package attribution also completed all `phoenix_ide` tests and all 535 active `phoenix-tools` tests with zero persistent servers. Real tmux tests briefly daemonize during execution as expected; their owners returned the host to zero after each completed suite.

After final Codex hardening, the owner no longer depends on inherited descriptors: the detached watchdog verifies its original parent relationship, observes an atomic cleanup-request marker or parent death, and requires five quiet exact-root probes before success. Cleanup command failure preserves the root and fails the test. The final full check again held `legacy_orphans=0`, `owner_servers=0`, and `ptys=168` before and after. The complete single/concurrent/five-repeat cross-crate matrix also remained exactly `ptys=168 test_servers=0 test_roots=0` after every phase.
