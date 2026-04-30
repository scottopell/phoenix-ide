# Bash Tool — Design Document

## Overview

The bash tool executes shell commands as pipe-backed children of the Phoenix
process. The execution model is unchanged at the kernel level (fork → exec
→ pipes); what changes from prior revisions is everything around it: output
goes into a per-handle ring buffer, the agent's call returns when its wait
window elapses (not when the process is killed), and a handle keeps the
process addressable for `peek` / `wait` / `kill` for the rest of the
conversation's lifetime.

Tmux-backed processes (TTY, persistence across Phoenix restart) are out of
scope here; see `specs/tmux-integration/`.

## Tool Surface (REQ-BASH-002, REQ-BASH-003, REQ-BASH-010)

### JSON Schema

```json
{
  "type": "object",
  "properties": {
    "cmd":  { "type": "string",  "description": "Shell command to execute via bash -c (spawn)" },
    "wait_seconds": { "type": "integer", "minimum": 0, "maximum": 900,
                      "description": "How long to block for the foreground answer (default 30)" },

    "peek": { "type": "string", "description": "Handle id to peek" },
    "wait": { "type": "string", "description": "Handle id to wait on" },
    "kill": { "type": "string", "description": "Handle id to kill" },

    "signal": { "type": "string", "enum": ["TERM", "KILL"],
                "description": "Signal to send (kill only); default TERM" },
    "lines":  { "type": "integer", "minimum": 1,
                "description": "Tail mode: return last N lines" },
    "since":  { "type": "integer", "minimum": 0,
                "description": "Incremental mode: return lines from offset K" },

    "mode": { "type": "string", "enum": ["default", "slow", "background"],
              "description": "DEPRECATED — alias for wait_seconds; will be removed" }
  },
  "oneOf": [
    { "required": ["cmd"]  },
    { "required": ["peek"] },
    { "required": ["wait"] },
    { "required": ["kill"] }
  ]
}
```

The `oneOf` clause makes mutual exclusion structural at the schema level;
runtime check (`OperationKindMutuallyExclusive` in `bash.allium`) is a belt
for backends that don't validate `oneOf`.

### Operation Modes

| Provided key | Operation | Required peers | Optional peers |
|---|---|---|---|
| `cmd`  | spawn | — | `wait_seconds`, `lines`/`since` (response window) |
| `peek` | peek | — | `lines` xor `since` |
| `wait` | wait | `wait_seconds` | `lines` xor `since` |
| `kill` | kill | — | `signal` (default TERM) |

`mode` (deprecated) is honoured only on spawn calls. Mapping:

| `mode` value | Equivalent `wait_seconds` |
|---|---|
| `default` | 30 |
| `slow` | 900 |
| `background` | 0 |

When `mode` is supplied, the response includes a `_deprecation` field:
`"the 'mode' parameter is deprecated; use 'wait_seconds' instead"`.

### Description Template (REQ-BASH-009, REQ-BASH-010)

```
Executes shell commands via bash -c, capturing combined stdout/stderr.
Bash state changes (working dir, variables, aliases) don't persist between calls.

Modes (exactly one per call):
  cmd=<string>     spawn a new command. Combine with wait_seconds (default 30):
                   how long to wait for the foreground answer. If the command
                   exits in time, you receive the exit code and final output.
                   If it doesn't, you receive a handle and the process keeps
                   running — peek, wait, or kill it later.
  peek=<handle>    return the current ring buffer state for a handle.
                   Use lines=N for the last N lines, or since=K for lines
                   after offset K.
  wait=<handle>    block up to wait_seconds for an existing handle to exit.
  kill=<handle>    terminate a handle. Default signal is TERM; signal=KILL for
                   immediate. After KILL_GRACE_SECONDS of TERM not taking,
                   automatic escalation to KILL.

For commands that need a TTY, interactive input, or that should survive
Phoenix restarts, use the tmux tool instead.

IMPORTANT: Keep commands concise. The cmd input must be < 60k tokens.
For complex scripts, write them to a file first and execute the file.

<pwd>{working_directory}</pwd>
```

## ToolContext Extension (REQ-BASH-014)

The `ToolContext` already exposes `browser()` for browser session access. Add
an analogous accessor for the bash handle registry:

```rust
#[derive(Clone)]
pub struct ToolContext {
    pub cancel: CancellationToken,
    pub conversation_id: String,
    pub working_dir: PathBuf,
    browser_sessions: Arc<BrowserSessionManager>,
    bash_handles: Arc<BashHandleRegistry>,
}

impl ToolContext {
    pub fn bash_handles(&self) -> BashHandleScope<'_> {
        BashHandleScope::new(&self.bash_handles, &self.conversation_id)
    }
}
```

`BashHandleScope` is a thin helper that:
- Uses `conversation_id` as a structural key for every operation, making
  cross-conversation lookups impossible to express (REQ-BASH-014).
- Holds whatever locking / sharing strategy the registry uses internally so
  the tool's call sites stay simple.

## In-Memory Handle Registry

```rust
pub struct BashHandleRegistry {
    inner: Arc<DashMap<ConversationId, ConversationHandles>>,
    db: Arc<DbConn>,
}

struct ConversationHandles {
    next_id: AtomicU64,
    live: HashMap<HandleId, Arc<LiveHandle>>,
    tombstones: HashMap<HandleId, Tombstone>,  // populated lazily from SQLite
}

pub struct LiveHandle {
    handle_id: HandleId,           // "b-1", "b-2", ...
    conversation_id: ConversationId,
    cmd: String,
    started_at: SystemTime,
    pgid: i32,
    child: Mutex<Option<tokio::process::Child>>,  // taken by the wait task
    ring: Mutex<RingBuffer>,
    next_offset: AtomicU64,
    exit_signal: tokio::sync::Notify,             // pulsed on ProcessExited
    exit_state: OnceLock<ExitState>,              // set once when wait fires
}

pub struct RingBuffer {
    lines: VecDeque<RingLine>,
    bytes_used: usize,
    bytes_cap: usize,
    start_offset: u64,
}

pub struct RingLine {
    offset: u64,
    bytes: Bytes,
}

pub struct Tombstone {
    handle_id: HandleId,
    cmd: String,
    final_cause: FinalCause,
    exit_code: Option<i32>,
    duration_ms: u64,
    finished_at: SystemTime,
    final_tail: Vec<RingLine>,    // bounded by TOMBSTONE_TAIL_LINES
    next_offset_at_exit: u64,
}

pub enum FinalCause {
    Exited,
    Killed,
    LostInRestart { shutdown_marker_ts: SystemTime },
}
```

Tombstones live in memory after demotion (so subsequent peeks are O(1)) and
are also persisted to SQLite for the `lost_in_restart` scenario. On startup,
the SQLite store is read into the in-memory map; a record marked
`lost_in_restart` is materialised as a `Tombstone` with `FinalCause::LostInRestart`
and an empty `final_tail` (we don't persist live ring contents to SQLite in
v1; see `BashHandleOutputDiskSpill` deferred entry in `bash.allium`).

## Spawn Flow (REQ-BASH-001, REQ-BASH-002, REQ-BASH-005, REQ-BASH-007, REQ-BASH-011)

```
agent → BashTool::run(input, ctx)
        ├─ parse + validate input (mutual exclusion, ranges)
        ├─ if mode supplied: map to wait_seconds + set _deprecation
        ├─ if not spawn: dispatch to peek/wait/kill handlers
        │
        └─ spawn path:
            ├─ bash_check::check(cmd) — REQ-BASH-011
            │     ┌─ reject → command_safety_rejected error
            │     └─ ok    → continue
            ├─ ctx.bash_handles().reserve_slot(conversation_id)
            │     ┌─ count >= LIVE_HANDLE_CAP → handle_cap_reached error
            │     └─ slot reserved with handle_id allocation
            ├─ persist tombstone shadow (status=running) to SQLite
            ├─ spawn child process (group leader)
            │     ┌─ spawn fails → spawn_failed error; rollback shadow record
            │     └─ ok          → child handle in hand
            ├─ register LiveHandle in conversation map
            ├─ spawn three async tasks:
            │     reader_stdout: read pipe → ring (with exit_signal pulse on EOF)
            │     reader_stderr: read pipe → ring (same)
            │     waiter:        Child::wait().await → run demotion → pulse exit_signal
            ├─ race tokio::select! between:
            │     waiter exit_signal → SpawnResponseExited
            │     sleep(wait_seconds) → SpawnResponseStillRunning
            └─ return response (handle remains live regardless)
```

Pseudocode:

```rust
async fn spawn(&self, cmd: &str, wait_seconds: u64, ctx: &ToolContext) -> ToolOutput {
    if let Err(reason) = bash_check::check(cmd) {
        return ToolOutput::error("command_safety_rejected", reason);
    }

    let scope = ctx.bash_handles();
    let slot = match scope.reserve_slot() {
        Ok(s) => s,
        Err(LiveHandles { live }) => {
            return ToolOutput::error_with_fields(
                "handle_cap_reached",
                json!({ "cap": LIVE_HANDLE_CAP, "live_handles": live, "hint": HINT_TEXT }),
            );
        }
    };

    let shadow_record = TombstoneRecord::initial(&slot.handle_id, cmd, &slot.conversation_id);
    self.db.write_tombstone(&shadow_record).await?;

    let mut command = tokio::process::Command::new("bash");
    command.args(["-c", cmd])
        .current_dir(&ctx.working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe { command.pre_exec(|| { libc::setpgid(0, 0); Ok(()) }); }

    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            self.db.delete_tombstone(&slot.handle_id).await.ok();
            scope.release_reserved(&slot);
            return ToolOutput::error("spawn_failed", e.to_string());
        }
    };

    let live = Arc::new(LiveHandle::new(slot, cmd.to_string(), child.id().unwrap() as i32));
    scope.commit(live.clone(), child);

    // wait window race
    tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => self.respond_cancel(live).await,
        _ = live.exit_signal.notified() => self.respond_exited(live, ctx).await,
        _ = tokio::time::sleep(Duration::from_secs(wait_seconds)) => {
            self.respond_still_running(live, wait_seconds * 1000, ctx).await
        }
    }
}
```

The reader/waiter tasks are spawned inside `scope.commit` so the registry's
internal locking owns their lifecycle.

### Reader task

```rust
async fn read_loop(pipe: ChildStdout|ChildStderr, ring: Arc<Mutex<RingBuffer>>,
                   next_offset: Arc<AtomicU64>) {
    let mut buf = BytesMut::with_capacity(4096);
    loop {
        match pipe.read_buf(&mut buf).await {
            Ok(0) => break,
            Ok(_) => split_lines_into_ring(&mut buf, &ring, &next_offset),
            Err(e) => { tracing::debug!(?e, "pipe read error"); break; }
        }
    }
}
```

`split_lines_into_ring` splits incoming bytes on `\n`, assigning each
complete line a fresh offset and appending it to the ring under the mutex. A
trailing partial line is held in `buf` until the next read brings the
newline. On EOF, any held partial bytes are flushed as a final line
regardless of trailing newline (otherwise output without a final newline
would be invisible).

### Waiter task

```rust
async fn wait_for_exit(child: tokio::process::Child, live: Arc<LiveHandle>,
                       db: Arc<DbConn>) {
    let exit_status = child.wait().await;  // reaps the zombie
    let final_cause = match exit_status {
        Ok(s) if s.code().is_some() => FinalCause::Exited,
        Ok(_) => FinalCause::Killed,         // killed by signal; no code
        Err(_) => FinalCause::Exited,        // shouldn't happen post-wait
    };
    let exit_code = exit_status.ok().and_then(|s| s.code());

    let tombstone = live.demote_to_tombstone(final_cause, exit_code);
    db.update_tombstone(&tombstone).await.ok();
    live.exit_signal.notify_waiters();
}
```

`demote_to_tombstone` runs under the same lock that guards the live ring.
After the call returns, the live ring is gone and the tombstone is
populated. `LiveHandle` re-shapes itself by an internal swap; the
`Arc<LiveHandle>` reference held by the registry now points at a structure
where the relevant fields read from the tombstone.

In Rust, this is cleanest as two Arc-pointed states:

```rust
pub enum HandleState {
    Live(LiveData),
    Tombstoned(Tombstone),
}

pub struct Handle {
    handle_id: HandleId,
    conversation_id: ConversationId,
    cmd: String,
    started_at: SystemTime,
    state: ArcSwap<HandleState>,
    exit_signal: tokio::sync::Notify,
}
```

`ArcSwap` lets the demotion swap state from `Live` to `Tombstoned` without
holding a write lock against in-flight peek operations.

## Peek (REQ-BASH-003, REQ-BASH-004)

```rust
async fn peek(&self, handle_id: &str, read_args: ReadArgs, ctx: &ToolContext) -> ToolOutput {
    let scope = ctx.bash_handles();
    let handle = match scope.lookup(handle_id) {
        Some(h) => h,
        None => return self.lookup_persisted_or_not_found(handle_id, ctx).await,
    };
    self.shape_response(handle, read_args, /*tombstone_only*/ false).await
}

fn shape_response(&self, handle: Arc<Handle>, read_args: ReadArgs, ...) -> Response {
    match handle.state.load().as_ref() {
        HandleState::Live(live) => {
            let ring = live.ring.lock();
            let window = read_window(&ring, &read_args);
            Response::running(handle.handle_id.clone(), window)
        }
        HandleState::Tombstoned(t) => {
            let window = read_window_from_tail(&t.final_tail, &read_args);
            Response::tombstoned(handle.handle_id.clone(), &t, window)
        }
    }
}
```

`read_window` honours `lines=N` (last N lines from the back) and `since=K`
(slice from the first line whose offset >= K, up to the tail) and computes
`truncated_before` from the ring's `start_offset` versus the requested view.

`lookup_persisted_or_not_found` covers the case where the handle is not in
memory but exists in the SQLite shadow store as `lost_in_restart`. The
record is materialised into an in-memory `Tombstoned` handle (with empty
`final_tail`) on first lookup so subsequent peeks are O(1).

## Wait (REQ-BASH-003)

`wait` is `peek` with a blocking phase up front:

```rust
async fn wait(&self, handle_id: &str, wait_seconds: u64, read_args: ReadArgs,
              ctx: &ToolContext) -> ToolOutput {
    let scope = ctx.bash_handles();
    let handle = match scope.lookup(handle_id) {
        Some(h) => h,
        None => return self.lookup_persisted_or_not_found(handle_id, ctx).await,
    };

    if matches!(*handle.state.load().as_ref(), HandleState::Tombstoned(_)) {
        return self.shape_response(handle, read_args, true).await;
    }

    tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => self.respond_cancel(handle).await,
        _ = handle.exit_signal.notified() => {
            // demotion ran; load the now-tombstoned state
            self.shape_response(handle, read_args, true).await
        }
        _ = tokio::time::sleep(Duration::from_secs(wait_seconds)) => {
            // still running — same handle returned
            self.shape_response(handle, read_args, false).await
                .with_status_still_running(wait_seconds * 1000)
        }
    }
}
```

The same `handle_id` is returned on re-timeout — the agent never
accumulates handles by repeated waits.

## Kill (REQ-BASH-003)

```rust
async fn kill(&self, handle_id: &str, signal: KillSignal, ctx: &ToolContext) -> ToolOutput {
    let scope = ctx.bash_handles();
    let handle = match scope.lookup(handle_id) {
        Some(h) => h,
        None => return self.lookup_persisted_or_not_found(handle_id, ctx).await,
    };

    let live = match handle.state.load().as_ref() {
        HandleState::Live(live) => live.clone(),
        HandleState::Tombstoned(_) => {
            return self.shape_response(handle, ReadArgs::default(), true)
                .await
                .with_already_terminal(true);
        }
    };

    let pgid = live.pgid;
    send_signal_to_group(pgid, signal.as_libc()).ok();
    let signal_sent = signal;

    let escalated = tokio::select! {
        _ = handle.exit_signal.notified() => None,
        _ = tokio::time::sleep(Duration::from_secs(KILL_GRACE_SECONDS)) => {
            if signal == KillSignal::TERM {
                send_signal_to_group(pgid, libc::SIGKILL).ok();
                handle.exit_signal.notified().await;
                Some(KillSignal::KILL)
            } else {
                handle.exit_signal.notified().await;
                None
            }
        }
    };

    self.shape_response(handle, ReadArgs::default(), true).await
        .with_kill_metadata(signal_sent, escalated)
}
```

`send_signal_to_group` sends to `-pgid` so all descendants of the original
shell child are signalled, not just the bash that ran `bash -c "..."`.

## SQLite Tombstone Shadow Store (REQ-BASH-007)

```sql
-- migration: bash_tombstones
CREATE TABLE bash_tombstones (
    handle_id          TEXT NOT NULL,
    conversation_id    TEXT NOT NULL,
    cmd                TEXT NOT NULL,
    status             TEXT NOT NULL,          -- 'running' | 'exited' | 'killed' | 'lost_in_restart'
    started_at         INTEGER NOT NULL,       -- unix ms
    finished_at        INTEGER,                -- unix ms; null while running
    duration_ms        INTEGER,                -- null while running
    exit_code          INTEGER,                -- nullable always; signal-killed has no code
    final_cause        TEXT,                   -- null while running
    final_tail_json    TEXT,                   -- json array of {offset, bytes_b64}; null while running
    shutdown_marker_ts INTEGER,                -- only for lost_in_restart
    PRIMARY KEY (conversation_id, handle_id)
);
CREATE INDEX bash_tombstones_status ON bash_tombstones(status);
```

A separate `phoenix_runtime_state` row records the most recent graceful
shutdown timestamp; reconciliation reads it for `shutdown_marker_ts`.

### Reconciliation on startup

```rust
async fn reconcile_bash_tombstones(db: &DbConn) -> anyhow::Result<()> {
    let last_shutdown = db.read_runtime_state("phoenix_shutdown_at").await?;
    let marker = last_shutdown.unwrap_or_else(|| SystemTime::now());

    db.execute("
        UPDATE bash_tombstones
        SET status = 'lost_in_restart',
            shutdown_marker_ts = ?,
            final_cause = 'lost_in_restart'
        WHERE status = 'running'
    ", &[marker.unix_ms()]).await?;

    Ok(())
}
```

This runs in the bedrock startup sequence before any tool routes accept
calls, so the very first peek after restart sees consistent state.

### Hard-delete cascade (REQ-BASH-006)

```sql
-- in conversation hard-delete transaction
DELETE FROM bash_tombstones WHERE conversation_id = ?;
```

The matching in-memory cleanup runs during the same delete handler (kill any
live processes in the conversation, drop the `ConversationHandles` map
entry).

## Error Envelope (REQ-BASH-008)

```json
{
  "error": "<stable_id>",
  "error_message": "<human-readable description>",
  "...": "<error-specific structured fields>"
}
```

All error identifiers (per REQ-BASH-008): `handle_not_found`,
`handle_cap_reached`, `wait_seconds_out_of_range`,
`peek_args_mutually_exclusive`, `command_safety_rejected`,
`spawn_failed`, `mutually_exclusive_modes`.

Error-specific fields (representative subset):

```json
// handle_cap_reached
{
  "error": "handle_cap_reached",
  "error_message": "this conversation has reached the cap of 8 live handles",
  "cap": 8,
  "live_handles": [
    { "handle": "b-3", "cmd": "cargo test", "age_seconds": 1820, "status": "running" }
  ],
  "hint": "kill or wait on a handle, or use the tmux tool for long-runners"
}

// command_safety_rejected
{
  "error": "command_safety_rejected",
  "error_message": "permission denied: blind git add commands ...",
  "reason": "blind_git_add"
}

// peek_args_mutually_exclusive
{
  "error": "peek_args_mutually_exclusive",
  "error_message": "specify exactly one of lines or since"
}
```

A successful tool call where the *command* failed (non-zero exit) is **not**
an error envelope. It is a normal response with `status: "exited"` and the
non-zero `exit_code`. The agent distinguishes by checking for the `error`
key versus the `status` key.

## Output Capture and Display (REQ-BASH-015)

The display-simplification rules from the prior revision (strip redundant
`cd <path> &&` prefix when path matches the conversation's working
directory; preserve `||` operator suffixes) apply unchanged to the `cmd`
field on spawn responses.

For non-spawn operations, the UI displays a synthetic command label rather
than a real shell command:

| Operation | Display |
|---|---|
| spawn       | `<simplified cmd>` |
| peek (live) | `peek b-7` |
| peek (tomb) | `peek b-7 (exited 22.0s, code 0)` |
| wait        | `wait b-7 (up to 30s)` |
| kill (TERM) | `kill b-7 (TERM)` |
| kill (KILL) | `kill b-7 (KILL)` |

The `display` field is computed by the tool result formatter, not the LLM
input layer.

## Command Safety Checks (REQ-BASH-011)

Unchanged from the prior revision. `src/tools/bash_check.rs` parses the
command via `brush-parser` and walks the AST for dangerous patterns. The
spawn path calls `bash_check::check(cmd)` before reserving a handle slot;
peek/wait/kill paths bypass this check (they operate on already-spawned
handles whose original command was already vetted).

The traversal handles pipelines, and/or chains, compound commands (loops,
conditionals, subshells, brace groups), and function bodies, recursing
through every nested SimpleCommand. `sudo` prefixes are stripped before
checking. Error messages suggest alternatives:

- `permission denied: blind git add commands (git add -A, git add ., git add --all, git add *) are not allowed, specify files explicitly`
- `permission denied: git push --force is not allowed. Use --force-with-lease for safer force pushes, or push without force`
- `permission denied: this rm command could delete critical data (.git, home directory, or root). Specify the full path explicitly (no wildcards, ~, or $HOME)`

## Landlock Enforcement (REQ-BASH-012, REQ-BASH-013)

Unchanged from the prior revision. Explore-mode conversations spawn child
processes with Landlock restrictions applied via the `landlock` crate at
`pre_exec` time. Work-mode conversations spawn without restrictions. The
detection of Landlock availability runs once at startup and is cached on
the registry. On unsupported kernels or non-Linux OSes, Explore-mode
read-only enforcement degrades to advisory tool-level checks; Work mode
operates normally.

The handle model does not change Landlock semantics. A handle's process
inherits the sandbox policy from its spawn-time mode; the policy is fixed
for the process's lifetime regardless of subsequent peek/wait/kill calls.

## Testing Strategy

### Unit tests
- Ring buffer line splitting, eviction, offset assignment, `truncated_before`
  computation under various read patterns (lines=N, since=K, both at
  edges).
- Tombstone demotion: live → tombstoned via `ArcSwap`; tail truncation at
  `TOMBSTONE_TAIL_LINES`; offset preservation across the boundary.
- Schema validation: mutual exclusion, `oneOf` enforcement, `mode` aliasing,
  range checks on `wait_seconds`.
- Error envelope shapes for each stable error id.

### Integration tests
- Spawn → exits within wait_seconds → `status: "exited"` with exit code.
- Spawn → wait_seconds elapses → `status: "still_running"` with handle.
- Repeated `wait` on same handle returns same handle id on each re-timeout.
- `kill` with TERM that doesn't take → auto-escalates to KILL within
  `KILL_GRACE_SECONDS`; response shows `signal_escalated: "KILL"`.
- `kill` on already-exited handle → `already_terminal: true`, no signal sent.
- Hard-delete a conversation with live handles → processes killed, SQLite
  records gone, in-memory entries gone.
- Phoenix shutdown with live handles → SQLite records remain `running`;
  startup reconciliation rewrites them to `lost_in_restart` with
  `shutdown_marker_ts` from the previous shutdown record.
- Phoenix SIGKILL'd (no graceful shutdown record) → reconciliation uses
  startup time as the marker.
- Peek on `lost_in_restart` handle → returns the structured tombstone
  response (status, marker, no final_tail).
- Cap rejection: spawn while at cap returns `handle_cap_reached` with the
  full live-handles list.
- Cross-conversation handle access: a handle id from conv A used in conv B
  returns `handle_not_found` (does not leak existence).

### Property tests
- `peek(since=end_offset)` after any sequence of writes never re-reads
  content (offset monotonicity).
- For any sequence of spawn/peek/wait/kill within `LIVE_HANDLE_CAP`, the
  in-memory and SQLite state remain consistent (tombstone shadow exists for
  every handle, terminal state implies tombstone, no live handle has an
  exited shadow record).
- `read_window` over a partially-evicted ring sets `truncated_before=true`
  iff the requested view extends below the current `start_offset`.

### Command Safety Tests (REQ-BASH-011)
Unchanged: 42 unit tests covering git add, git push, rm patterns, sudo
handling, pipelines, compound commands, edge cases. 4 integration tests
verifying the check runs before spawn:
- `test_blocked_git_add`
- `test_blocked_rm_rf_root`
- `test_blocked_git_push_force`
- `test_allowed_command_runs`

## File Organization

```
src/tools/
├── mod.rs               # Tool registry, trait definitions
├── bash.rs              # BashTool dispatch (spawn/peek/wait/kill)
├── bash/
│   ├── handle.rs        # Handle, LiveData, Tombstone, ArcSwap-backed state
│   ├── ring.rs          # RingBuffer, RingLine, eviction
│   ├── registry.rs      # BashHandleRegistry, ConversationHandles
│   ├── reader.rs        # stdout/stderr → ring tasks
│   ├── waiter.rs        # Child::wait → demotion task
│   ├── kill.rs          # signal sending + auto-escalation logic
│   └── shadow.rs        # SQLite tombstone read/write/reconciliation
├── bash_check.rs        # Command safety checks (REQ-BASH-011)
├── patch.rs             # PatchTool
├── patch/               # Patch tool internals
└── ...
```

`bash.rs` is the dispatch layer: parse input, route to spawn/peek/wait/kill.
The handle/registry/ring/reader/waiter/kill modules under `bash/` carry the
state-machine implementation of `bash.allium`.

## Migration from Prior Revision

The old `mode` enum survives as an alias on the schema for one or two
releases. Tool-call SSE wire records from older sessions remain readable —
their schemas didn't include the new fields, so deserialisation tolerates
absence. The tool's *response* shape is new: callers (UI, eval harness,
generated TypeScript via `ts_rs`) must update.

Specifically:
- Old success response: `{ output: string, exit_code: int }`.
- New success response: `{ status, handle, exit_code, duration_ms,
  start_offset, end_offset, truncated_before, lines, ... }`.

The TS codegen lives under `ui/src/generated/` (per `AGENTS.md` § TypeScript
codegen). After this design lands, run `./dev.py codegen` and update the
valibot schemas in `ui/src/sseSchemas.ts`. The display layer in `ui/`
needs to handle the new `still_running` status and the `display` field
variants for peek/wait/kill.

The bedrock state machine sees only one externally-visible change: a tool
result with `status: "still_running"` and a handle is a normal tool result,
not an error. No new bedrock state; no new transition. Tool-result handling
in `runtime/` may need to surface the still-running status to the UI in a
visually distinct way (e.g., a "running" badge on the tool call card with a
peek button), but that is a UI concern, not a state-machine concern.
