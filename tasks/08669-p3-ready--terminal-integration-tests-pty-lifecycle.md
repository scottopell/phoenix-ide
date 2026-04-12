---
created: 2026-04-11
priority: p3
status: ready
artifact: src/terminal/
---

# Terminal integration tests: PTY I/O paths and lifecycle teardown

The terminal unit tests (src/terminal/proptests.rs) cover the registry invariants,
parser safety, resize validation, and environment construction. The following
spec rules have no test coverage because they require real PTY/process infrastructure
that doesn't exist in the current test suite.

## Untested paths

### PTY I/O paths (require real fork+pty)
- `PtyOutputForwarded` (REQ-TERM-004, REQ-TERM-010): reader task feeds parser and
  WebSocket in same handler with no gaps — structural guarantee only, not verified
- `ShellExited` (REQ-TERM-007, REQ-TERM-009): EIO handling, waitpid, zombie prevention
- `UserClosedTerminal` (REQ-TERM-008): master_fd drop → SIGHUP → shell exit chain
- `UserInputForwarded` (REQ-TERM-004): 0x00 data frame written to master fd

### Lifecycle teardown (require real executor + broadcast channel)
- `TerminalAbandonedWithConversation` (REQ-TERM-012): `ConversationBecameTerminal`
  fires from executor → ws.rs subscription removes from registry → Task A/B exit
- `emit_terminal_lifecycle_event()` in executor.rs: the broadcast send is never
  observed in any test

### read_terminal quiescence path
- `wait_for_quiescence=true`: the watch channel counter comparison and 5s timeout
  are untested because they require a live quiescence_tx sender

## Suggested approach

1. **PTY smoke test binary**: add `src/bin/pty_smoke.rs` that spawns a PTY,
   writes `echo OK\n`, reads back the output, and asserts `OK` appears. This
   verifies the core spawn→I/O→EIO chain without a WebSocket.

2. **Integration test with mock WS**: use `tokio_tungstenite` in test mode or
   a pair of `tokio::sync::mpsc` channels to simulate the WebSocket, then drive
   a full terminal session lifecycle against a real PTY.

3. **read_terminal quiescence unit test**: mock the quiescence_tx by creating a
   TerminalHandle manually (using /dev/null as master_fd), sending a tick on
   quiescence_tx, and verifying the tool returns promptly.
