# Terminal: Design Document

## Conceptual Foundation

The canonical reference for the PTY/TTY stack is Linus Åkesson's article:
https://www.linusakesson.net/programming/tty/

The Rust API server occupies the role that iTerm2 or xterm occupies in a native
terminal — it holds the PTY master fd and mediates I/O. The browser (xterm.js)
renders the output and captures keystrokes. The kernel PTY subsystem handles echo,
line discipline, special character dispatch, and signal delivery. The shell knows
nothing about any of this.

## WebSocket Authentication (REQ-TERM-013)

The terminal WebSocket endpoint uses the same session mechanism as all other API
endpoints. The session is validated during the HTTP upgrade handshake — before
the WebSocket is accepted and before any PTY is created.

```
GET /api/conversations/{id}/terminal  (upgrade: websocket)
  ├── Middleware: validate session cookie / Authorization header
  ├── If invalid: return HTTP 401, no PTY created
  ├── If valid: proceed with WebSocket upgrade
  └── Then: check conversation ownership + 409 guard (see below)
```

In axum, middleware runs before the WebSocket handler function is entered, so
authentication failure returns a plain HTTP response without ever calling
`ws.on_upgrade()`.

**Auth mechanism:** Password authentication is implemented via `src/api/auth.rs`
(task `08642-p2-done--basic-password-auth`). When `PHOENIX_PASSWORD` is set,
all API requests require auth via `phoenix-auth` cookie or Bearer token, using
constant-time comparison. When unset, auth is bypassed (backward compatible).
The terminal WebSocket upgrade request goes through the same axum middleware
layer — no terminal-specific auth needed. REQ-TERM-013 is satisfied.

## Active Terminal Registry and 409 Guard (REQ-TERM-003)

To enforce exactly one terminal per conversation, the runtime holds a registry
of active terminal sessions:

```rust
// In RuntimeState or equivalent shared state:
active_terminals: HashMap<ConversationId, TerminalHandle>
```

On each WebSocket upgrade (after auth validation):

```rust
if active_terminals.contains_key(&conversation_id) {
    return StatusCode::CONFLICT; // 409
}
// Otherwise: spawn PTY, insert into registry, proceed
active_terminals.insert(conversation_id, handle);
```

On teardown (EIO or WebSocket disconnect):

```rust
active_terminals.remove(&conversation_id);
```

The registry must be protected by a `Mutex` or equivalent. The check-and-insert
must be atomic relative to other callers to avoid TOCTOU races on rapid
reconnect.

```
Browser (xterm.js)
     ↕  WebSocket (binary frames)
Rust API (WebSocket handler + PTY I/O tasks)
     ↕  PTY master fd
Kernel PTY subsystem + line discipline
     ↕  PTY slave fd
Shell process ($SHELL -i, cwd = conversation directory)
```

### I/O Is Fully Decoupled

There are two independent loops:

```
Input:  xterm.js onData → WebSocket frame → Rust handler → write() to master fd
Output: read() from master fd → Rust read loop → WebSocket frame → xterm.js render
```

These share nothing except the fd. Echo is the kernel line discipline injecting bytes
into the output stream — not the input loop responding to the output loop. The shell
does not echo. The shell does not handle `Ctrl-C`. The line discipline does all of that.

### What the Line Discipline Owns

- **Echo** (`ECHO` flag): writes typed bytes back to master fd so they appear on screen
- **Cooked mode buffering** (`ICANON`): holds input until Enter, flushes to slave fd
- **Special character dispatch**: `Ctrl-C` → `SIGINT`, `Ctrl-Z` → `SIGTSTP`,
  `Ctrl-D` → EOF — none reach the shell as bytes
- **Output translation** (`ONLCR`): `\n` → `\r\n` on the way out
- **Raw mode**: when vim calls `tcsetattr()` to clear `ICANON`+`ECHO`, bytes flow
  through immediately with no processing

## Spawn Path

### PTY Creation (REQ-TERM-001, REQ-TERM-002)

Use the `nix` crate directly — not `portable-pty` — to preserve the learning value
of each syscall.

```
openpty()           →  master_fd + slave_fd
fork()              →  child gets slave, parent keeps master
  [child]
    setsid()            new session; child is session leader
    ioctl(TIOCSCTTY)    slave_fd becomes controlling terminal
    dup2(slave_fd, 0)   slave is stdin
    dup2(slave_fd, 1)   slave is stdout
    dup2(slave_fd, 2)   slave is stderr
    close(slave_fd)     fd already dup'd, close the original
    chdir(cwd)          conversation working directory
    execvp($SHELL, ["-i"])  exec the shell; explicit env (see below)
  [parent]
    close(slave_fd)     parent must not hold slave open
    store master_fd     → TerminalHandle
```

### Environment Construction (REQ-TERM-002)

Construct explicitly. Never inherit the API server's environment:

```rust
let env: &[(&str, &str)] = &[
    ("TERM",      "xterm-256color"),  // critical — wrong/missing breaks readline
    ("COLORTERM", "truecolor"),       // xterm.js supports it; tools key off it
    ("HOME",      &user_home),
    ("USER",      &username),
    ("SHELL",     &user_shell),
    ("PATH",      &user_path),
    ("LANG",      "en_US.UTF-8"),
];
```

`TERM=xterm-256color` is the single most important variable. Wrong or missing:
readline degrades, arrow keys print raw escape sequences, vim and htop misbehave.

### TerminalHandle

```rust
struct TerminalHandle {
    master_fd: OwnedFd,
    child_pid:  Pid,
}
// Drop closes master_fd → kernel delivers SIGHUP to shell's process group
```

Dropped on WebSocket close. The drop must be explicit and exhaustive — panics and
leaked fds leave orphan shells.

## WebSocket Protocol (REQ-TERM-004)

Binary frames throughout. Text frames corrupt arbitrary PTY bytes.

```
Byte 0   Meaning          Direction          Remaining bytes
0x00     PTY data         bidirectional      raw bytes
0x01     Resize           client → server    u16be cols, u16be rows
```

The type byte is stripped before writing to master fd (data frames) or before
calling `ioctl` (resize frames).

## Async I/O Model

PTY master fds are pollable on Linux and work with tokio's reactor.

```
WebSocket connection accepted
  ├── Spawn PTY + shell (sync, in spawn_blocking or before async handoff)
  ├── Task A: loop { read(master_fd) → WebSocket binary frame + vt100 parser }
  │     EIO → clean shutdown (not error)
  └── Task B: loop { WebSocket frame → write(master_fd) OR ioctl(TIOCSWINSZ) }
```

Both tasks hold an `Arc<TerminalHandle>`. Either task exiting signals the other
to shut down. Output channel is bounded (`output_channel_bound = 4096` bytes)
to provide backpressure against fast producers (`yes`, large `cat`).

### EIO Handling (REQ-TERM-007)

When the slave fd closes (shell exits), `read()` from master returns `EIO`.
This is PTY EOF — not an error. Treat as clean termination:

```rust
Err(e) if e.raw_os_error() == Some(libc::EIO) => {
    tracing::debug!("PTY EOF (shell exited)");
    break; // clean shutdown
}
```

Do NOT log at `error` level. Every normal terminal close produces EIO.

## Terminal Resize (REQ-TERM-005, REQ-TERM-006)

### Initial Resize

Send immediately on WebSocket open, before the shell produces its first prompt:

```rust
// After WS upgrade, before entering the read loop:
let dims = wait_for_initial_resize_frame(&mut ws).await?;
apply_resize(&master_fd, &mut parser, dims);
```

xterm.js FitAddon computes cols/rows from the DOM and sends a resize frame as its
first message on connect.

### Resize Application

```rust
fn apply_resize(master_fd: &OwnedFd, parser: &mut vt100::Parser, dims: Dims) {
    let ws = libc::winsize {
        ws_col: dims.cols as u16,
        ws_row: dims.rows as u16,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe { libc::ioctl(master_fd.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
    parser.set_size(dims.rows, dims.cols); // keep parser in sync
    // Kernel delivers SIGWINCH to foreground process group automatically
}
```

## vt100 Parser / Scraping Layer (REQ-TERM-010, REQ-TERM-011)

A `vt100::Parser` instance runs server-side alongside the output path:

```rust
// Task A: same bytes going to WebSocket also feed the parser
let bytes = read_master_fd(&master_fd).await?;
ws_sender.send(binary_frame(0x00, &bytes)).await?;
parser.process(&bytes);  // in-order, no gaps
```

**Critical invariants:**
- Parser initialized with the same `(rows, cols)` as the terminal.
- Resized on every `TIOCSWINSZ` call (same `apply_resize` function).
- Every byte sent to WebSocket also goes to parser, in order, with no gaps.
  The parser is a state machine — dropped or reordered bytes corrupt its state
  permanently for that session.

### Quiescence Debounce

For agent reads, PTY quiescence (output stream quiet for 300ms) is a reliable
signal that a command has completed. The `read_terminal` tool may be called at any
time; callers should prefer reading after quiescence for meaningful output.

### `read_terminal` Tool

The tool fetches the current screen state from the parser:

```rust
// read_terminal tool run():
let text = parser.screen().contents();
ToolOutput::success(text)
```

The tool returns an error if no terminal is active for the conversation.

## Session Teardown

### User Closes Terminal (REQ-TERM-008, REQ-TERM-009)

1. WebSocket closes → `TerminalHandle` dropped
2. `Drop` closes `master_fd`
3. Kernel delivers `SIGHUP` to shell's process group
4. Shell exits → slave fd closes → Task A reads `EIO`
5. Task A calls `waitpid(child_pid)` → reaps child, prevents zombie
6. Task A signals Task B to shut down

### Shell Exits First (REQ-TERM-007)

1. Shell exits → slave fd closes → Task A reads `EIO`
2. Task A closes WebSocket, reaps child
3. WebSocket close → `TerminalHandle` dropped → `master_fd` closed

### Conversation Terminates (REQ-TERM-012)

When a conversation reaches a terminal state, the terminal session is torn down
via the same master fd close → SIGHUP chain.

## Crate Choices

| Purpose | Crate | Status | Notes |
|---|---|---|---|
| PTY syscalls | `nix` | ✅ In Cargo.toml | Add `pty` to features: `features = ["signal", "process", "pty"]` |
| vt100 parsing | `vt100` | ❌ Not present | Add to Cargo.toml |
| WebSocket | `axum` | ⚠️ Feature missing | Add `"ws"` to axum features: `features = ["macros", "ws"]` |
| Async runtime | `tokio` | ✅ In Cargo.toml | No changes needed |

Do NOT use `portable-pty` — it abstracts away the syscalls that make the PTY
stack concrete and understandable.

## Known Gotchas

### Process Lifecycle
- **Orphan shells**: SIGHUP only fires if the master fd is actually closed. Panics
  and leaked fds leave orphan shells. `Drop` must be explicit and exhaustive.
- **Zombies**: `waitpid` must be called after shell exit. The reader task (Task A)
  is the correct place — it observes EIO first.
- **EIO on shell exit**: handle as clean termination. Logging as error produces
  noise on every normal close.

### Sizing
- **Initial size race**: xterm.js sends resize as its first frame; wait for it before
  the shell produces output.
- **vt100 parser out of sync**: call `parser.set_size()` on every `TIOCSWINSZ`.

### WebSocket
- **Binary frames only**: text frames corrupt arbitrary PTY bytes.
- **Backpressure**: bound the output channel. A fast producer (`yes`, large `cat`)
  can generate bytes faster than the WebSocket drains.

### Environment
- **Secret leakage**: construct env explicitly; never inherit the API server's
  full environment.
- **macOS**: PTY master fds have historically had kqueue quirks. macOS support
  is deferred to a follow-on task — test on macOS before shipping there.
