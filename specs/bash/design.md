# Bash Tool — Design Document

## Overview

The bash tool executes shell commands as pipe-backed children of the Phoenix
process. The execution model is unchanged at the kernel level (fork → exec
→ pipes); what changes from prior revisions is everything around it: output
goes into a per-handle ring buffer, the agent's call returns when its wait
window elapses (not when the process is killed), and a handle keeps the
process addressable for `peek` / `wait` / `kill` for the rest of the Phoenix
process lifetime.

Tmux-backed processes (TTY, persistence across Phoenix restart) are out of
scope here; see `specs/tmux-integration/`. Handles in this spec are
in-memory only — they do not survive Phoenix restart.

## Tool Surface (REQ-BASH-002, REQ-BASH-003, REQ-BASH-010)

### JSON Schema

```json
{
  "type": "object",
  "properties": {
    "cmd":  { "type": "string",  "description": "Shell command to execute via bash -c (spawn). Will be wrapped as `bash -c \"exec <cmd>\"` so the bash process replaces itself with the user command and exit signals propagate cleanly." },
    "wait_seconds": { "type": "integer", "minimum": 0, "maximum": 900,
                      "description": "How long this single tool call blocks before handing back a handle (default 30). This is NOT a process kill timeout: the process is NEVER killed when wait_seconds elapses; it keeps running and you receive a handle. Use kill=<handle> to actually terminate." },

    "peek": { "type": "string", "description": "Handle id to peek" },
    "wait": { "type": "string", "description": "Handle id to wait on" },
    "kill": { "type": "string", "description": "Handle id to kill" },

    "signal": { "type": "string", "enum": ["TERM", "KILL"],
                "description": "Signal to send (kill only); default TERM. Sent exactly once; no auto-escalation." },
    "lines":  { "type": "integer", "minimum": 1,
                "description": "Tail mode: return last N lines" },
    "since":  { "type": "integer", "minimum": 0,
                "description": "Incremental mode: return lines from offset K" },

    "mode": { "type": "string", "enum": ["default", "slow", "background"],
              "description": "DEPRECATED — alias for wait_seconds; removed in the second Phoenix release after this revision lands." }
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
for backends that don't validate `oneOf`. Dual-pass `mode + wait_seconds` is
rejected at runtime via `mutually_exclusive_modes` with
`conflicting_args: ["mode", "wait_seconds"]` and a `recommended_action`
field directing the agent to drop the deprecated `mode` parameter.

### Operation Modes

| Provided key | Operation | Required peers | Optional peers |
|---|---|---|---|
| `cmd`  | spawn | — | `wait_seconds`, `lines`/`since` (response window) |
| `peek` | peek | — | `lines` xor `since` |
| `wait` | wait | `wait_seconds` | `lines` xor `since` |
| `kill` | kill | — | `signal` (default TERM) |

`mode` (deprecated) is honored only on spawn calls and only when
`wait_seconds` is absent. Mapping when accepted:

| `mode` value | Equivalent `wait_seconds` |
|---|---|
| `default` | 30 |
| `slow` | 900 |
| `background` | 0 |

When `mode` is supplied alongside `wait_seconds`, the call fails with
`mutually_exclusive_modes` (with `conflicting_args` and
`recommended_action`) rather than silently picking one. When
`mode` is supplied alone, the response includes a `deprecation_notice`
field stating the removal version explicitly. The field name is
deliberately not underscore-prefixed (no `_deprecation`): leading-
underscore is widely read by LLMs as "metadata; ignore," which is
the opposite of the signal we want — the agent should attend to
this field and migrate.

### Description Template (REQ-BASH-009, REQ-BASH-010)

```
Executes shell commands via bash -c, capturing combined stdout/stderr.
Bash state changes (working dir, variables, aliases) don't persist between calls.

Modes (exactly one per call):

  cmd=<string>     Spawn a new command. wait_seconds (default 30) is NOT a
                   timeout — the process is NEVER killed when wait_seconds
                   elapses. wait_seconds only controls how long this single
                   tool call blocks before handing you back a handle so you
                   can do other work. The process keeps running in the
                   background until it exits naturally or you call
                   kill=<handle>. A response with status="still_running"
                   means the process is alive and will stay alive — peek
                   it later, wait on it, or kill it explicitly.

  peek=<handle>    Return the current ring buffer state for a handle.
                   Use lines=N for the last N lines, or since=K for lines
                   after offset K. status="tombstoned" in the response
                   means the handle's process has finished — the
                   final_cause field tells you how (exited normally, or
                   killed by signal). status="kill_pending_kernel" means
                   the kill signal you sent was delivered but the process
                   is in uninterruptible kernel sleep — peek again later;
                   sending kill again with the same signal does NOT
                   compound (signals don't queue that way), but you can
                   escalate by sending kill with signal=KILL.

  wait=<handle>    Block up to wait_seconds for an existing handle to exit.
                   If wait_seconds elapses first, the SAME handle is
                   returned with status="still_running" — never accumulate
                   handles by repeated waits. If the handle has already
                   finished, returns immediately with status="tombstoned".

  kill=<handle>    Terminate a handle. Default signal is TERM; signal=KILL
                   for immediate. The signal is sent EXACTLY ONCE; this
                   tool does not auto-escalate TERM to KILL after a grace
                   period. If your TERM doesn't take effect within
                   ~30 seconds, the response is status="kill_pending_kernel"
                   and you decide whether to escalate by calling kill
                   again with signal=KILL. (Don't retry with signal=TERM:
                   the kernel doesn't queue duplicate signals; the original
                   TERM is still pending and a second TERM is a no-op.)

If you peek a handle and get error="handle_not_found", it likely means
Phoenix restarted between when you spawned the process and now — bash
handles do NOT survive Phoenix process restart. For processes that need
to survive Phoenix restart, that need a TTY, that need stdin, or that
are interactive REPLs, use the tmux tool instead.

IMPORTANT: Keep commands concise. The cmd input must be < 60k tokens.
For complex scripts, write them to a file first and execute the file.

<pwd>{working_directory}</pwd>
```

The negation-based framing (`NOT a timeout` … `is NEVER killed` … `EXACTLY
ONCE` … `does not auto-escalate`) is load-bearing. Affirmative descriptions
get pattern-matched into the POSIX `timeout(1)` / `kill PID` priors;
explicit negations override those priors.

## ToolContext Extension (REQ-BASH-014)

The `ToolContext` already exposes `browser()` returning `Result<Arc<RwLock<BrowserSession>>, BrowserError>`. The bash handle accessor matches that shape:

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
    pub async fn bash_handles(&self) -> Result<Arc<RwLock<ConversationHandles>>, BashHandleError> {
        self.bash_handles.get_or_create(&self.conversation_id).await
    }
}
```

Returning `Arc<RwLock<...>>` rather than a lifetime-bound guard composes
cleanly with `ToolContext: Clone` and matches the established pattern.
Per-conversation handles never appear in two places: `get_or_create`
returns the same `Arc` for the same conversation id.

## Child Process Reaper (REQ-BASH-007)

Phoenix sets up the reaper at startup and runs the kill-tree at shutdown:

```rust
pub fn install_reaper() {
    #[cfg(target_os = "linux")]
    {
        use libc::{prctl, PR_SET_CHILD_SUBREAPER};
        let rc = unsafe { prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
        if rc != 0 {
            tracing::warn!("PR_SET_CHILD_SUBREAPER failed; orphaned descendants will reparent to init");
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        tracing::warn!("PR_SET_CHILD_SUBREAPER unavailable on this OS; \
                        descendants that escape their process group may leak on Phoenix exit");
    }
}

pub async fn shutdown_kill_tree(registry: &BashHandleRegistry) {
    let live_pgids: Vec<i32> = registry.snapshot_live_pgids().await;
    for pgid in &live_pgids {
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
    }
    tokio::time::sleep(Duration::from_secs(SHUTDOWN_KILL_GRACE_SECONDS)).await;
}
```

Why this matters: the prior draft assumed Phoenix dying would cascade
SIGHUP to its children. That is wrong on Linux for processes that don't
share a controlling terminal — SIGHUP is a TTY-hangup signal, not a parent-
death signal. Without the subreaper bit, double-forked daemons (`(cmd &) &`),
programs that call `setsid`, and any descendant that resets its own pgid
will outlive Phoenix and leak. With the subreaper bit set, escapees
reparent to Phoenix rather than init; the shutdown kill-tree pass then
SIGKILLs them along with the immediate group.

`install_reaper()` is called from the Phoenix startup sequence before any
tool routes accept calls. `shutdown_kill_tree()` is wired into the same
shutdown handler that closes the database and stops the SSE relay.

The kill-tree is `SIGKILL`, not `SIGTERM`, because Phoenix is exiting
anyway — graceful-shutdown handlers in the children would race with
Phoenix's exit. `SHUTDOWN_KILL_GRACE_SECONDS` (default 2) is the time
Phoenix waits for the kernel to deliver the exits before returning control.

## In-Memory Handle Registry

```rust
pub struct BashHandleRegistry {
    inner: Arc<DashMap<ConversationId, Arc<RwLock<ConversationHandles>>>>,
}

pub struct ConversationHandles {
    next_id: u64,
    live: HashMap<HandleId, Arc<Handle>>,
    tombstones: HashMap<HandleId, Arc<Handle>>,
}

pub struct Handle {
    handle_id: HandleId,
    conversation_id: ConversationId,
    cmd: String,
    started_at: SystemTime,
    state: RwLock<Arc<HandleState>>,
    exit_signal: tokio::sync::watch::Sender<Option<ExitState>>,
    exit_observer: tokio::sync::watch::Receiver<Option<ExitState>>,
}

pub enum HandleState {
    Live(LiveData),
    Tombstoned(Tombstone),
}

pub struct LiveData {
    pgid: i32,
    ring: Mutex<RingBuffer>,
    next_offset: u64,
}

pub struct Tombstone {
    final_cause: FinalCause,
    exit_code: Option<i32>,
    duration_ms: u64,
    finished_at: SystemTime,
    final_tail: Vec<RingLine>,
    next_offset_at_exit: u64,
    kill_attempted_at: Option<SystemTime>,
    kill_signal_sent: Option<KillSignal>,
}

pub enum FinalCause {
    Exited,
    Killed,
    Signaled,           // external signal (oom-killer, external `kill -9`)
    KillPendingKernel,  // Phoenix-sent kill, process not yet exited
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
```

`RwLock<Arc<HandleState>>` rather than `ArcSwap` is the deliberate choice:
contention is bounded (per-conversation cap is 8 handles, peek is brief,
demotion is once-per-process), the existing browser pattern uses `RwLock`,
and `ArcSwap`'s hazard-pointer model has surprising `Drop`-ordering
behavior that doesn't pull its weight here.

`tokio::sync::watch::channel<Option<ExitState>>` rather than `Notify` for
the exit signal: `Notify::notify_waiters` only wakes tasks already parked
in `notified()`; tasks that call `notified().await` after the notification
fired block forever. `watch` channels are always observable — a late
subscriber sees the most recent value. `OnceLock<ExitState>` would also
work; `watch` was picked because in-flight wait responses use
`changed().await` naturally.

## Spawn Flow (REQ-BASH-001, REQ-BASH-002, REQ-BASH-005, REQ-BASH-011)

```
agent → BashTool::run(input, ctx)
        ├─ parse + validate input (oneOf, mode-vs-wait_seconds conflict, ranges)
        ├─ if mode supplied (and wait_seconds absent): map to wait_seconds + set deprecation_notice
        ├─ if not spawn: dispatch to peek/wait/kill handlers
        │
        └─ spawn path:
            ├─ bash_check::check(cmd) — REQ-BASH-011
            │     ┌─ reject → command_safety_rejected error
            │     └─ ok    → continue
            ├─ ctx.bash_handles().await? → Arc<RwLock<ConversationHandles>>
            ├─ ConversationHandles::reserve_slot()
            │     ┌─ count >= LIVE_HANDLE_CAP → handle_cap_reached error
            │     └─ slot reserved with handle_id allocation
            ├─ spawn child process (group leader)
            │     ┌─ spawn fails → spawn_failed error; release reservation
            │     └─ ok          → child handle in hand
            ├─ register Handle with state = Live(LiveData {...})
            ├─ spawn three async tasks:
            │     reader_stdout: read pipe → ring (split on '\n', append RingLine)
            │     reader_stderr: read pipe → ring (same)
            │     waiter:        Child::wait().await → run demotion → publish exit on watch
            ├─ race tokio::select! between:
            │     waiter exit_observer.changed() → SpawnExitsWithinWait
            │     sleep(wait_seconds) → SpawnExceedsWait
            │     ctx.cancel.cancelled() → respond_cancel
            └─ return response (handle remains live regardless of which arm won)
```

Pseudocode:

```rust
async fn spawn(&self, cmd: &str, wait_seconds: u64, ctx: &ToolContext) -> ToolOutput {
    if let Err(reason) = bash_check::check(cmd) {
        return ToolOutput::error("command_safety_rejected", reason);
    }

    let handles_arc = ctx.bash_handles().await?;
    let mut handles = handles_arc.write().await;
    let slot = match handles.reserve_slot() {
        Ok(s) => s,
        Err(LiveHandlesFull { live }) => {
            return ToolOutput::error_with_fields(
                "handle_cap_reached",
                json!({ "cap": LIVE_HANDLE_CAP, "live_handles": live, "hint": HINT_TEXT }),
            );
        }
    };

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
            handles.release_reserved(&slot);
            return ToolOutput::error("spawn_failed", e.to_string());
        }
    };

    let pgid = child.id().unwrap() as i32;
    let (exit_tx, exit_rx) = tokio::sync::watch::channel(None);
    let handle = Arc::new(Handle::new(slot, cmd.to_string(), pgid, exit_tx, exit_rx.clone()));
    handles.commit_live(handle.clone());
    drop(handles); // release write lock before spawning long-lived tasks

    spawn_reader_tasks(&handle, child.stdout.take(), child.stderr.take());
    spawn_waiter_task(handle.clone(), child);

    let mut exit_rx = exit_rx;
    tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => self.respond_cancel(handle).await,
        Ok(_) = exit_rx.changed() => self.respond_exited(handle, ctx).await,
        _ = tokio::time::sleep(Duration::from_secs(wait_seconds)) => {
            self.respond_still_running(handle, wait_seconds * 1000, ctx).await
        }
    }
}
```

### Reader task

```rust
async fn read_loop(pipe: ChildStdout|ChildStderr, handle: Arc<Handle>) {
    let mut buf = BytesMut::with_capacity(4096);
    loop {
        match pipe.read_buf(&mut buf).await {
            Ok(0) => break,
            Ok(_) => split_lines_into_ring(&mut buf, &handle),
            Err(e) => { tracing::debug!(?e, "pipe read error"); break; }
        }
    }
}
```

`split_lines_into_ring` splits incoming bytes on `\n`, assigns each
complete line a fresh offset (under the live ring's mutex), and appends
to the ring. A trailing partial line is held in `buf` until the next
read brings the newline. On EOF, any held partial bytes are flushed as a
final line regardless of trailing newline.

### Waiter task

```rust
async fn wait_for_exit(child: tokio::process::Child, handle: Arc<Handle>) {
    // Panic guard: if anything below panics, publish a sentinel to the
    // exit watch so wait/spawn callers don't hang forever.
    let panic_guard = ExitWatchPanicGuard::new(handle.clone());

    let exit_status = child.wait().await;
    let (final_cause, exit_code, signal_number) = match &exit_status {
        Ok(s) => {
            // We spawn user commands as `bash -c "exec <cmd>"` so the bash
            // process replaces itself with the user code; ExitStatus then
            // reflects the user code's exit, not bash's. WIFSIGNALED gives
            // us the signal number directly; otherwise bash's 128+signum
            // convention also lets us recover signal info from the exit code.
            if let Some(sig) = s.signal() {
                (FinalCause::Killed, None, Some(sig))
            } else if let Some(code) = s.code() {
                let sig = if code > 128 && code < 128 + 64 {
                    Some(code - 128)
                } else {
                    None
                };
                (FinalCause::Exited, Some(code), sig)
            } else {
                (FinalCause::Exited, None, None)
            }
        }
        Err(_) => (FinalCause::Exited, None, None),
    };

    handle
        .transition_to_terminal(final_cause, exit_code, signal_number)
        .await;
    let _ = handle.exit_signal.send(Some(ExitState::from_terminal(&handle).await));
    panic_guard.disarm();
}

struct ExitWatchPanicGuard {
    handle: Option<Arc<Handle>>,
}

impl ExitWatchPanicGuard {
    fn new(handle: Arc<Handle>) -> Self { Self { handle: Some(handle) } }
    fn disarm(mut self) { self.handle = None; }
}

impl Drop for ExitWatchPanicGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // Waiter task panicked; publish a sentinel so any wait() calls
            // unblock with an explicit error rather than hanging forever.
            let _ = handle.exit_signal.send(Some(ExitState::WaiterPanicked));
            tracing::error!(handle_id = %handle.handle_id,
                            "bash waiter task panicked; handle marked WaiterPanicked");
        }
    }
}
```

`transition_to_terminal` is the **single helper** through which all
HandleState writes pass — both the waiter task (this code) and the kill
response timer's `mark_kill_pending_kernel` (in the kill flow below) go
through it. The helper acquires the `RwLock<Arc<HandleState>>` for
write, reads current state, and:

- If current state is already terminal (`exited` or `killed`), it is a
  no-op (the waiter already won the race against the timer; the timer's
  late kill_pending_kernel write is dropped).
- If current state is `running` or `kill_pending_kernel` and the
  caller is the waiter, it snapshots the live ring's tail (last
  `TOMBSTONE_TAIL_LINES`), constructs the `Tombstone`, and swaps
  state from `Live` to `Tombstoned`. After the swap, the live ring is
  dropped and its memory freed.
- If current state is `running` and the caller is the kill response
  timer, it transitions to `kill_pending_kernel` and records the
  attempt timestamp + signal sent.

Funneling all writes through one helper closes the late-exit-vs-
timer race the panel surfaced. Without it, the timer could regress a
terminal state back to `kill_pending_kernel`, losing the exit_code
and final_tail.

### Watch-channel rule

Each `wait` / `spawn` call MUST clone a fresh receiver from
`handle.exit_observer` at call time and use that single receiver in
its `tokio::select!`. **Never reuse a receiver across calls** —
`watch::Receiver::changed()` only fires on transitions; if a receiver
already saw the terminal value, a second `changed().await` on it
hangs forever. The tombstone fast-path in `wait()` (return immediately
if state is already terminal) avoids this in the current design, but
a future refactor that removes the fast-path would surface the
footgun. Document this explicitly in test plans.

## Peek (REQ-BASH-003, REQ-BASH-004)

```rust
async fn peek(&self, handle_id: &str, read_args: ReadArgs, ctx: &ToolContext) -> ToolOutput {
    let handles_arc = ctx.bash_handles().await?;
    let handles = handles_arc.read().await;
    let Some(handle) = handles.lookup(handle_id) else {
        return ToolOutput::error("handle_not_found", json!({ "handle_id": handle_id }));
    };
    drop(handles);
    self.shape_response(handle, read_args).await
}

async fn shape_response(&self, handle: Arc<Handle>, read_args: ReadArgs) -> ToolOutput {
    let state_guard = handle.state.read().await;
    match state_guard.as_ref() {
        HandleState::Live(live) => {
            let ring = live.ring.lock();
            let window = read_window(&ring, &read_args);
            ToolOutput::structured("bash", running_response(handle.handle_id.clone(), window))
        }
        HandleState::Tombstoned(t) => {
            let window = read_window_from_tail(&t.final_tail, &read_args);
            ToolOutput::structured("bash", tombstoned_response(handle.handle_id.clone(), t, window))
        }
    }
}
```

`read_window` honors `lines=N` (last N lines from the back) and `since=K`
(slice from the first line whose offset >= K, up to the tail) and computes
`truncated_before` from the ring's `start_offset` versus the requested
view.

A handle that's not in the live or tombstone tables returns
`handle_not_found` immediately. Cross-conversation handle ids return the
same response — the lookup is conversation-keyed.

## Wait (REQ-BASH-003)

```rust
async fn wait(&self, handle_id: &str, wait_seconds: u64, read_args: ReadArgs,
              ctx: &ToolContext) -> ToolOutput {
    let handles_arc = ctx.bash_handles().await?;
    let handles = handles_arc.read().await;
    let Some(handle) = handles.lookup(handle_id) else {
        return ToolOutput::error("handle_not_found", json!({ "handle_id": handle_id }));
    };
    drop(handles);

    if matches!(*handle.state.read().await.as_ref(), HandleState::Tombstoned(_)) {
        return self.shape_response(handle, read_args).await;
    }

    let mut exit_rx = handle.exit_observer.clone();
    tokio::select! {
        biased;
        _ = ctx.cancel.cancelled() => self.respond_cancel(handle).await,
        Ok(_) = exit_rx.changed() => self.shape_response(handle, read_args).await,
        _ = tokio::time::sleep(Duration::from_secs(wait_seconds)) => {
            self.shape_response(handle, read_args).await
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
    let handles_arc = ctx.bash_handles().await?;
    let handles = handles_arc.read().await;
    let Some(handle) = handles.lookup(handle_id) else {
        return ToolOutput::error("handle_not_found", json!({ "handle_id": handle_id }));
    };
    drop(handles);

    let live = match handle.state.read().await.as_ref() {
        HandleState::Live(live) => live.clone(),
        HandleState::Tombstoned(_) => {
            // Already terminal — return tombstoned response shape.
            // status: "tombstoned" itself conveys the no-signal-sent
            // case; no `already_terminal` flag needed.
            return self.shape_tombstoned_response(handle, ReadArgs::default()).await;
        }
    };

    let pgid = live.pgid;
    send_signal_to_group(pgid, signal.as_libc()).ok();

    let mut exit_rx = handle.exit_observer.clone();
    tokio::select! {
        biased;
        Ok(_) = exit_rx.changed() => {
            self.shape_response(handle, ReadArgs::default())
                .await
                .with_kill_metadata(signal, /*pending=*/ false)
        }
        _ = tokio::time::sleep(Duration::from_secs(KILL_RESPONSE_TIMEOUT_SECONDS)) => {
            handle.mark_kill_pending_kernel(signal).await;
            self.shape_response(handle, ReadArgs::default())
                .await
                .with_kill_metadata(signal, /*pending=*/ true)
        }
    }
}
```

The kill tool does **not** auto-escalate. If the agent passed `signal:
TERM` and 30 seconds elapse without the process exiting, the response
returns `status: "kill_pending_kernel"` with `signal_sent: "TERM"` and
the agent decides whether to call kill again with `signal: "KILL"`.

`mark_kill_pending_kernel` updates the live state to record `kill_attempted_at`
and `kill_signal_sent`. The waiter task survives — if the process
eventually exits (the kernel finally delivers, the frozen mount comes
back), `HandleProcessKilled` fires and the handle transitions
`kill_pending_kernel → killed`. A subsequent peek/wait/kill observes the
now-terminal state.

`send_signal_to_group(-pgid, ...)` signals the entire process group, not
just the bash that ran `bash -c "..."`. Combined with the subreaper bit
set at startup, this catches escapees that reparent to Phoenix.

## Hard-Delete Cascade (REQ-BASH-006)

Wired into the bedrock conversation-hard-delete handler:

```rust
async fn cascade_bash_on_delete(registry: &BashHandleRegistry, conv: &ConversationId) {
    let Some(handles_arc) = registry.remove(conv) else { return; };
    let handles = handles_arc.write().await;
    for (_, handle) in handles.live.iter() {
        if let HandleState::Live(live) = handle.state.read().await.as_ref() {
            let _ = unsafe { libc::kill(-live.pgid, libc::SIGKILL) };
        }
    }
    // tombstones drop with the registry entry; no SQLite to clean up
}
```

The conversation-hard-delete handler runs this synchronously alongside
the tmux server kill (`specs/tmux-integration/`) and any other per-
conversation cleanup. There is no SQLite shadow store to clean up;
in-memory tombstones are dropped along with the registry entry.

> **Bedrock dependency:** the hard-delete cascade integration is wired
> through `cascade_bash_on_delete`, called directly from the bedrock
> hard-delete handler per REQ-BED-032. No event-bus / subscriber pattern
> is involved.

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
`spawn_failed`, `mutually_exclusive_modes`. The dual-pass case
(`mode` + `wait_seconds`) is folded into `mutually_exclusive_modes`
with structured `conflicting_args` and `recommended_action` fields
rather than carrying its own stable id.

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

// mutually_exclusive_modes — operation-key conflict (cmd + peek, etc.)
{
  "error": "mutually_exclusive_modes",
  "error_message": "exactly one of cmd, peek, wait, kill must be provided",
  "conflicting_args": ["cmd", "peek"],
  "recommended_action": "remove one of the operation keys"
}

// mutually_exclusive_modes — mode/wait_seconds dual-pass
{
  "error": "mutually_exclusive_modes",
  "error_message": "the deprecated 'mode' parameter cannot be used with 'wait_seconds'; pass wait_seconds alone",
  "conflicting_args": ["mode", "wait_seconds"],
  "recommended_action": "remove the deprecated 'mode' parameter; pass 'wait_seconds' alone",
  "mode": "background",
  "wait_seconds": 30
}

// handle_not_found — with hint about Phoenix restart
{
  "error": "handle_not_found",
  "error_message": "handle b-7 not found in this conversation",
  "handle_id": "b-7",
  "hint": "if Phoenix restarted since this handle was created, the handle was lost — bash handles do not survive Phoenix restart. For processes that should survive Phoenix restart, use the tmux tool."
}
```

A successful tool call where the *command* failed (non-zero exit) is **not**
an error envelope. It is a normal response with `status: "tombstoned"`,
`final_cause: "exited"`, and the non-zero `exit_code`. The agent
distinguishes by checking for the `error` key versus the `status` key.

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
| kill (pending) | `kill b-7 (TERM, pending)` |

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
- Tombstone demotion: live → tombstoned via `RwLock<Arc<HandleState>>`
  swap; tail truncation at `TOMBSTONE_TAIL_LINES`; offset preservation
  across the boundary.
- Schema validation: `oneOf` enforcement, mutual exclusion, `mode` aliasing,
  `mode_and_wait_seconds_conflict` dual-pass rejection, range checks on
  `wait_seconds`.
- Error envelope shapes for each stable error id.

### Integration tests
- Spawn → exits within wait_seconds → `status: "exited"` with exit code.
- Spawn → wait_seconds elapses → `status: "still_running"` with handle.
- Repeated `wait` on same handle returns same handle id on each re-timeout.
- `kill` with TERM → process exits within timeout → `status: "tombstoned"`,
  `final_cause: "killed"`, `signal_sent: "TERM"`, `signal_number: 15`,
  no auto-escalation.
- `kill` with TERM → process does NOT exit within
  `KILL_RESPONSE_TIMEOUT_SECONDS` → `status: "kill_pending_kernel"`,
  `kill_signal_sent: "TERM"`. Subsequent `kill` with `signal: KILL`
  escalates explicitly.
- `kill` on already-exited handle → `status: "tombstoned"` (the status
  itself conveys the already-terminal case; no separate flag).
- External `kill -9` from outside Phoenix → handle reaches `status:
  "tombstoned"`, `final_cause: "killed"`, `signal_number: 9`. (Note:
  with the `bash -c "exec ..."` spawn pattern, the bash wrapper is
  replaced by the user command, so external signals reach the user
  command directly and surface in `Child::wait()`'s `ExitStatus::signal()`.)
- Late-arriving exit after `kill_pending_kernel`: handle transitions
  `kill_pending_kernel → killed` (or `→ exited`); subsequent peek/wait/
  kill observes the now-tombstoned state via `status: "tombstoned"`.
- Hard-delete a conversation with live handles → processes killed,
  in-memory entries gone.
- Phoenix shutdown with live handles → kill-tree pass SIGKILLs all groups;
  reaper bit is set so escapees are caught.
- Cap rejection: spawn while at cap returns `handle_cap_reached` with the
  full live-handles list.
- Cross-conversation handle access: a handle id from conv A used in conv B
  returns `handle_not_found` (does not leak existence).

### Property tests
- `peek(since=end_offset)` after any sequence of writes never re-reads
  content (offset monotonicity).
- For any sequence of spawn/peek/wait/kill within `LIVE_HANDLE_CAP`, the
  in-memory state is consistent (live count never exceeds cap, terminal
  state implies tombstone, no live handle has both representations).
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
├── bash.rs              # BashTool dispatch (spawn/peek/wait/kill)
├── bash/
│   ├── handle.rs        # Handle, LiveData, Tombstone, RwLock<Arc<HandleState>> swap
│   ├── ring.rs          # RingBuffer, RingLine, eviction
│   ├── registry.rs      # BashHandleRegistry, ConversationHandles
│   ├── reader.rs        # stdout/stderr → ring tasks
│   ├── waiter.rs        # Child::wait → demotion task; FinalCause discrimination
│   ├── kill.rs          # signal sending; KILL_RESPONSE_TIMEOUT; kill_pending_kernel transition
│   └── reaper.rs        # PR_SET_CHILD_SUBREAPER setup + shutdown_kill_tree
├── bash_check.rs        # Command safety checks (REQ-BASH-011)
├── patch.rs             # PatchTool
├── patch/               # Patch tool internals
└── ...
```

The project's clippy convention requires `foo.rs + foo/` rather than
`foo/mod.rs`; the layout above complies.

## Migration from Prior Revision

The old `mode` enum survives as an alias on the schema for one or two
releases (concretely: removed in the second Phoenix release after this
revision lands). Tool-call SSE wire records from older sessions remain
readable — their schemas didn't include the new fields, so deserialization
tolerates absence. The tool's *response* shape is new: callers (UI, eval
harness, generated TypeScript via `ts_rs`) must update.

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

The `subagent` tool propagates the bash schema to nested agents. Any
schema-passthrough logic that hardcoded the old `mode` enum needs to be
updated to also accept `wait_seconds` and the operation modes.

### Cross-cutting changes the migration touches

- `src/tools/bash.rs`: rewrite around dispatch + handle ops.
- `src/tools/bash/`: new module subtree.
- `src/tools.rs`: ToolContext gains `bash_handles()`; tool registry gains
  the new bash module wiring.
- `src/api/wire.rs`: bash response shape changes (SseWireEvent variants
  that carry tool results).
- `ui/src/generated/`: regenerated by `./dev.py codegen`.
- `ui/src/sseSchemas.ts`: valibot schemas for the new shape.
- `ui/src/components/.../tool-results/...`: new rendering for
  `still_running`, peek/wait/kill displays.
- `src/llm/mock.rs`: mocked bash responses in test fixtures need updating.
- `src/tools/subagent.rs`: schema passthrough.
- `phoenix-client.py`: any client-side parsing of bash output.
- `src/db/migrations/`: **no new migration** (we deliberately dropped the
  SQLite shadow store — see REQ-BASH-006 rationale).
- `src/runtime/`: hard-delete cascade gains a bash-cleanup step (depends
  on bedrock surfacing a hard-delete event; see Bedrock dependency above).
- Phoenix startup: `install_reaper()` call wired into the existing
  startup sequence.
- Phoenix shutdown: `shutdown_kill_tree()` wired into the existing
  shutdown handler.
