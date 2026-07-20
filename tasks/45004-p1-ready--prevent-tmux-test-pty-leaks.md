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

- [ ] Every test that can spawn tmux uses the centralized owner; no manual bottom-of-test `kill-server` cleanup remains.
- [ ] Panic/unwind cleanup is deterministic and tested.
- [ ] Forced test-process termination cleanup is deterministic and tested on macOS.
- [ ] `./dev.py check` cannot finish successfully while run-owned test tmux servers remain.
- [ ] Cleanup cannot target production/durable Phoenix tmux servers.
- [ ] Repeated tmux test runs do not increase run-owned tmux process or PTY counts.
- [ ] Existing tmux behavior tests continue to pass under parallel nextest execution.
