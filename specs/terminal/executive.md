# Terminal — Executive Summary

## What It Is

A real, PTY-backed terminal running the user's `$SHELL` in a browser tab. Not a
simulated shell, not a sandboxed environment — an actual pseudoterminal with the full
kernel line discipline, connected to xterm.js over a binary WebSocket. Interactive
programs (vim, htop, fzf), colour, readline, and dotfiles all work as they would in
iTerm2 or any native terminal.

## Why It Exists

PhoenixIDE organises work into conversations, each tied to a directory. The terminal
opens into that directory, giving immediate access to the environment the agent is
working in — without SSH, without switching applications.

A secondary motivation is learning: the implementation uses `nix` crate syscalls
directly (`openpty`, `fork`, `setsid`, `TIOCSCTTY`, `TIOCSWINSZ`) rather than a
higher-level abstraction, making the PTY/TTY stack concrete and auditable. The
canonical conceptual reference is https://www.linusakesson.net/programming/tty/.

## Scope

**Included in this spec:**
- PTY spawn path (fork, exec, setsid, env construction)
- WebSocket I/O relay (binary frames, type-prefixed protocol)
- WebSocket authentication (session identity precondition)
- Terminal resize (TIOCSWINSZ + SIGWINCH)
- Session lifecycle (spawn-on-connect, teardown-on-close, 409 on duplicate)
- Server-side vt100 screen parser (fed every byte in order)
- Output channel backpressure (bounded channel, no byte drops)
- `read_terminal` agent tool (parser-backed screen contents)
- Conversation teardown cascades to terminal

**Explicitly excluded / deferred:**
- UI panel placement (deferred to implementation)
- Scrollback persistence (ephemeral; tmux named sessions as future path)
- Shell integration markers / OSC 133 (v2 — structured command/output events)
- macOS PTY kqueue considerations (Linux target for v1)
- Multiple terminals per conversation (exactly one)
- PTY privilege escalation (API runs as same Unix user as shell)

## Key Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Max terminals per conversation | 1 (reject 409 if active) | Simple lifecycle, no ambiguity |
| WebSocket frame type | Binary only | PTY output is arbitrary bytes, not UTF-8 |
| Shell invocation | `$SHELL -i` | Interactive mode, sources rc files |
| Environment | Explicit construction + enumerated injection | No blind inheritance; only listed vars + the named `PtyEnvInjection` reach the child |
| Output persistence | Ephemeral | No DB writes; tmux integration deferred |
| Agent access | `read_terminal` tool | vt100 parser-backed, available in v1 |
| PTY crate | `nix` (not `portable-pty`) | Preserves learning value of each syscall |
| vt100 parser | `vt100` crate | Correct ANSI/VT100 screen state |
| Auth | Same session as rest of API | No extra mechanism needed |

## Build Order

1. **PTY smoke test** — standalone Rust binary: `openpty` + `fork` + bash, read/write
   master fd in a blocking loop. Verify interactive shell from the terminal.
2. **WebSocket wiring** — add WS endpoint to axum. Wire PTY I/O. Test with `websocat`.
3. **xterm.js frontend** — minimal xterm.js + FitAddon. Verify resize works.
4. **vt100 parser** — add scraping layer once basic terminal is stable.
5. **`read_terminal` tool** — expose parser contents to agent.
6. **Shell integration markers** — OSC 133 hooks for structured output (v2).

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| REQ-TERM-001: PTY-backed terminal per conversation | ✅ Done | src/terminal/spawn.rs |
| REQ-TERM-002: Explicit shell environment construction | ✅ Done | `phoenix-terminal/src/spawn.rs` `build_env`; injection via `PtyEnvInjection` (see `specs/command-suggestion`) |
| REQ-TERM-003: Exactly one terminal per WorkScope | ✅ Done | src/terminal/session.rs:ActiveTerminals (HashMap<WorkScope, …>) |
| REQ-TERM-004: Binary WebSocket framing | ✅ Done | src/terminal/ws.rs |
| REQ-TERM-005: Initial resize before first prompt | ✅ Done | src/terminal/ws.rs:wait_for_resize() |
| REQ-TERM-006: Resize propagated to PTY and parser | ✅ Done | src/terminal/ws.rs:apply_resize() |
| REQ-TERM-007: EIO treated as clean termination | ✅ Done | src/terminal/ws.rs:reader_task() |
| REQ-TERM-008: Master fd closed on WebSocket disconnect | Known contract drift; separate disposition required | `terminal_ws::detach_only` preserves the registered master and live child for reclaim, contrary to REQ-TERM-008 and Allium `UserClosedTerminal`. See disposition below. |
| REQ-TERM-009: Child process reaped after shell exit | Candidate; tests authored, execution unverified | `ActiveTerminals::install_child_observer`, `TerminalChildLifecycle`, `ChildExitReceipt`, and `ActiveTerminals::complete_retirement_by_observing`; detached cleanup and attached EOF-drain behavior require qualification. |
| REQ-TERM-010: vt100 parser fed every byte in order | ✅ Done | src/terminal/ws.rs:reader_task() |
| REQ-TERM-011: `read_terminal` agent tool | ✅ Done | src/tools/read_terminal.rs |
| REQ-TERM-012: Terminal torn down with WorkScope | ✅ Done | src/terminal/ws.rs:teardown watcher (per-scope dispatch) |
| REQ-TERM-013: WebSocket endpoint authentication | ✅ Done | src/api/auth.rs middleware |
| REQ-TERM-014: Output channel backpressure | ✅ Done | src/terminal/ws.rs:OUTPUT_BUF bounded reads |
| REQ-TERM-WS-001: Terminal sessions keyed by WorkScope | ✅ Done | src/terminal/session.rs:ActiveTerminals; src/work_scope.rs |

## Detached-child reaping candidate

The candidate installs one observer on successful registry publication through
`insert_reserved` or `try_insert_exact`. It reaps independently of attachment and
publishes a read-only in-memory receipt. Natural exit does not send `TearDown` to
an attached relay: the existing EOF path can drain output. Cleanup acquires
`attach_permit`, closes the master, and removes only the exact registered handle.
A live idle detached child is preserved. Explicit Close awaits the same receipt;
natural cleanup does not clear its retirement fence.

Coverage authored in `crates/phoenix-terminal/src/proptests.rs` includes both
publication paths, live-idle preservation, attached authority retention, stale
instance protection, observer failure, and Close/observer sharing. Real-PTY tests
include `detached_direct_shell_exit_is_reaped_without_reconnect_or_close` and
`complete_retirement_exits_and_reaps_long_lived_detached_direct_pty`. Existing
`terminal_ws::reclaim_tests` also exercise real PTY handoff and explicit retirement.
These are source-level coverage claims, **not passing-test or runtime acceptance
claims**; candidate execution remains unverified here. Task 08669 tracks scope
and remaining qualification.

## Separate disconnect-policy disposition

REQ-TERM-008 and Allium `UserClosedTerminal` require master closure on every
WebSocket disconnect. Actual `terminal_ws::detach_only` preserves the master and
child in `ActiveTerminals`, and `reclaim` reattaches to that session. This is a
known normative/implementation contradiction, not completed REQ-TERM-008 coverage.
The narrow detached-exit fix does not authorize changing disconnect policy.
Task 08669 records a separate disposition item: obtain the policy decision, then
reconcile the requirement, Allium rule, implementation, and tests together. No
normative open question or silent policy rewrite is introduced here.

Legacy `design.md` also describes reader-owned reaping and disconnect teardown;
it is not an accurate guide to this candidate's observer ownership. Its broader
lifecycle reconciliation belongs to that separate disposition, not this patch.
Descriptor inheritance and cancelled-spawn publication gaps are separately
recorded in task 08669 and are outside this candidate.
