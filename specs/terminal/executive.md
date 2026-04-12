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
| Environment | Explicit construction | No secret leakage from API server env |
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
| REQ-TERM-002: Explicit shell environment construction | ✅ Done | src/terminal/spawn.rs:build_env() |
| REQ-TERM-003: Exactly one terminal per conversation | ✅ Done | src/terminal/session.rs:ActiveTerminals |
| REQ-TERM-004: Binary WebSocket framing | ✅ Done | src/terminal/ws.rs |
| REQ-TERM-005: Initial resize before first prompt | ✅ Done | src/terminal/ws.rs:wait_for_resize() |
| REQ-TERM-006: Resize propagated to PTY and parser | ✅ Done | src/terminal/ws.rs:apply_resize() |
| REQ-TERM-007: EIO treated as clean termination | ✅ Done | src/terminal/ws.rs:reader_task() |
| REQ-TERM-008: Master fd closed on WebSocket disconnect | ✅ Done | src/terminal/session.rs:TerminalHandle::Drop |
| REQ-TERM-009: Child process reaped after shell exit | ✅ Done | src/terminal/ws.rs:waitpid() |
| REQ-TERM-010: vt100 parser fed every byte in order | ✅ Done | src/terminal/ws.rs:reader_task() |
| REQ-TERM-011: `read_terminal` agent tool | ✅ Done | src/tools/read_terminal.rs |
| REQ-TERM-012: Terminal torn down with conversation | ✅ Done | src/terminal/ws.rs:teardown watcher |
| REQ-TERM-013: WebSocket endpoint authentication | ✅ Done | src/api/auth.rs middleware |
| REQ-TERM-014: Output channel backpressure | ✅ Done | src/terminal/ws.rs:OUTPUT_BUF bounded reads |

**Progress:** 14 of 14 complete
