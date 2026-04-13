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
  ├── Task A: loop { read(master_fd) → WebSocket binary frame + command_tracker.ingest(&bytes) }
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
apply_resize(&master_fd, dims);
```

xterm.js FitAddon computes cols/rows from the DOM and sends a resize frame as its
first message on connect.

### Resize Application

```rust
fn apply_resize(master_fd: &OwnedFd, dims: Dims) {
    let ws = libc::winsize {
        ws_col: dims.cols as u16,
        ws_row: dims.rows as u16,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe { libc::ioctl(master_fd.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
    // Kernel delivers SIGWINCH to foreground process group automatically
}
```

## Command Tracker (REQ-TERM-010, REQ-TERM-021)

A `CommandTracker` runs server-side, fed every byte from the PTY output path.
It implements `vte::Perform` to capture structured command records via OSC 133
C/D markers.

### CommandRecord

```rust
struct CommandRecord {
    command_text: String,       // OSC 133;C payload (may be empty)
    output:       String,       // captured text, ANSI stripped; may include disk path if truncated
    exit_code:    Option<i32>,  // from OSC 133;D; None if D omits code
    started_at:   Timestamp,    // when C was processed
    duration_ms:  u64,          // milliseconds from C to D
}
```

### How CommandTracker implements vte::Perform

`vte` parses the byte stream and calls the `Perform` trait methods for each decoded
element. `CommandTracker` uses only three:

- `print(char)`: appends the character to the current output buffer when capturing
  (i.e. between a C and D marker). `vte` calls `print` only for printable characters
  — escape sequences never reach it. ANSI stripping is therefore structural, not
  a post-processing step.
- `execute(byte)`: appends a newline (`\n`) to the buffer when the byte is `0x0a`
  (LF) and capturing is active; all other control bytes are ignored.
- `osc_dispatch(params, bell_terminated)`: intercepts OSC 133 sequences. On a `C`
  marker, starts a new capture (stores command_text, records started_at, resets the
  output buffer). On a `D` marker, finalises the capture (computes duration_ms,
  records exit_code, applies truncation if needed, pushes to the ring buffer). `A`
  and `B` markers are accepted and ignored.

Everything else (`hook`, `put`, `unhook`, `csi_dispatch`, `esc_dispatch`) is a
no-op — `CommandTracker` has no grid, no cursor, and no resize state.

### Ring Buffer

```rust
struct CommandTracker {
    records:         VecDeque<CommandRecord>, // capacity 5
    current_capture: Option<CaptureState>,
    session_id:      SessionId,
    seq:             u64,
}
```

`VecDeque` with a fixed capacity of 5. When a 6th record would be pushed,
`pop_front` is called first to evict the oldest entry.

### Output Truncation

The 128KB threshold matches the bash tool's `MAX_OUTPUT_LENGTH` constant. When
`current_capture.output.len()` exceeds 128KB at finalisation:

1. Write the full output bytes to `~/.phoenix-ide/terminal-output/<session-id>/<seq>.txt`.
2. Store a truncated preview (first 4KB) with the disk path appended in the record's
   `output` field, in the same format as the bash tool's `truncate_output` helper.
3. Increment `seq` after each record so disk files are uniquely named.

### Placement in TerminalHandle

```rust
struct TerminalHandle {
    master_fd:       OwnedFd,
    child_pid:       Pid,
    command_tracker: Arc<Mutex<CommandTracker>>,
}
```

`command_tracker` is behind `Arc<Mutex<_>>` so both Task A (which calls
`command_tracker.ingest(&bytes)`) and the tool handlers (which read from the ring
buffer) can access it without shared mutable state.

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
| OSC 133 parsing and output capture | `vte` | ❌ Not present | Add to Cargo.toml; implement `Perform` trait on `CommandTracker` |
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

### WebSocket
- **Binary frames only**: text frames corrupt arbitrary PTY bytes.
- **Backpressure**: bound the output channel. A fast producer (`yes`, large `cat`)
  can generate bytes faster than the WebSocket drains.

### Environment
- **Secret leakage**: construct env explicitly; never inherit the API server's
  full environment.
- **macOS**: PTY master fds have historically had kqueue quirks. macOS support
  is deferred to a follow-on task — test on macOS before shipping there.
