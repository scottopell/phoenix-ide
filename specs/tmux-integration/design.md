# Tmux Integration — Design Document

## Overview

The tmux integration is two cooperating pieces:

1. The agent-facing `tmux` tool — a pass-through that prepends
   `-S <conv-sock>` and forwards the rest of the args to the tmux binary.
2. The terminal feature's tmux-attach behaviour — when the tmux binary is
   available, the in-app terminal's PTY runs `tmux attach` against the
   conversation's socket instead of the user's `$SHELL` directly.

Both share a per-conversation tmux server registry that handles socket-path
resolution, lazy spawn, stale-socket detection, and the conversation-hard-
delete cascade. The registry is reachable via `ToolContext.tmux()` for the
agent tool and consumed directly by the terminal session-spawn path.

## Tool Surface (REQ-TMUX-003, REQ-TMUX-009, REQ-TMUX-012)

### JSON Schema

```json
{
  "type": "object",
  "required": ["args"],
  "properties": {
    "args": {
      "type": "array",
      "items": { "type": "string" },
      "description": "Subcommand and its arguments, e.g. [\"new-window\", \"-d\", \"-n\", \"serve\", \"./serve\"]"
    },
    "wait_seconds": {
      "type": "integer", "minimum": 1, "maximum": 900,
      "description": "Max seconds to block on the subprocess (default 30)"
    }
  }
}
```

### Description Template

```
Invokes tmux against this conversation's dedicated socket. The full tmux CLI
is available; provide the subcommand + flags as `args`.

This conversation's tmux server is isolated from every other conversation
and from any tmux server you may have running on the host: the socket path
is fixed by Phoenix and cannot be overridden by passing -L or -S in args.
If you do pass them, tmux will reject the duplicate server-selection flag
with a usage error.

Common subcommands:
  new-window -d -n NAME COMMAND     spawn a new window running COMMAND
  list-windows                       enumerate windows in the current session
  capture-pane -p -t NAME -S -2000   read up to 2000 lines of scrollback
                                     for window NAME
  send-keys -t NAME "input" Enter    send input to a window
  kill-window -t NAME                terminate a window
  kill-server                        terminate this conversation's tmux server
                                     (rare; conversation hard-delete does
                                      this automatically)

Use this tool — not bash — for processes that:
  * need a TTY (REPLs, programs that detect isatty)
  * should survive Phoenix process restart
  * you want to interact with via stdin
  * are servers, watchers, or other long-lived processes

Use bash for one-shot non-interactive commands.

Note: this tool's response shape differs from the bash tool. Bash returns
status/handle/exit_code/lines; this tool returns
status/exit_code/duration_ms/stdout/stderr/truncated. stdout and stderr
are kept SEPARATE here because tmux subcommands emit structured CLI
output where the distinction matters (capture-pane to stdout, warnings
to stderr).

Persistence is across Phoenix restart only, not system reboot. After a
host reboot, this server's state is lost; the next operation creates a
fresh server.
```

### Response Shape

```json
{
  "status": "ok" | "timed_out" | "cancelled",
  "exit_code": <int | null>,
  "duration_ms": <int>,
  "stdout": "<string>",
  "stderr": "<string>",
  "truncated": <bool>
}
```

`stdout` and `stderr` are returned separately because tmux subcommands
emit structured CLI output where the distinction matters.

## Per-Conversation Tmux Server Registry

```rust
pub struct TmuxRegistry {
    inner: Arc<DashMap<ConversationId, Arc<RwLock<TmuxServer>>>>,
    socket_dir: PathBuf,    // ~/.phoenix-ide/tmux-sockets/
    binary_available: bool, // discovered once at startup
}

pub struct TmuxServer {
    conversation_id: ConversationId,
    socket_path: PathBuf,
    state: ServerState,
}

pub enum ServerState {
    NotProbed,          // initial, before any operation
    Live,
    Gone,               // post-hard-delete; entry is dropped from registry
}
```

`RwLock<TmuxServer>` rather than `ArcSwap`: matches the established
pattern from `specs/bash/` (`RwLock<ConversationHandles>`) and from the
existing browser session manager. `ArcSwap` was proposed in an earlier
draft, but the panel review flagged that it's a novel concurrency
primitive (not used elsewhere in the codebase), its hazard-pointer model
has surprising `Drop`-ordering behavior, and contention here is bounded
(probe + spawn run at most once per operation).

### Socket Path Resolution

```rust
fn socket_path_for(socket_dir: &Path, conversation_id: &ConversationId) -> PathBuf {
    // ~/.phoenix-ide/tmux-sockets/conv-<id>.sock
    socket_dir.join(format!("conv-{}.sock", conversation_id))
}
```

Conversation IDs are filename-safe (per bedrock contracts); no escaping
needed. The socket directory is created on registry initialisation with
permissions 0700.

### Probe (REQ-TMUX-005, REQ-TMUX-006)

```rust
pub enum ProbeResult {
    Live,
    DeadSocket,   // socket file exists but `tmux ls` failed
    NoSocket,     // socket file absent
}

async fn probe(socket_path: &Path) -> ProbeResult {
    if !socket_path.exists() {
        return ProbeResult::NoSocket;
    }
    let status = tokio::process::Command::new("tmux")
        .args(["-S", &socket_path.to_string_lossy(), "ls"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    match status {
        Ok(s) if s.success() => ProbeResult::Live,
        _ => ProbeResult::DeadSocket,
    }
}
```

`-S` takes an absolute path, not a label, so there's no `TMUX_TMPDIR`
interaction to worry about. The path the agent sees in `tmux ls` output
is literally the file Phoenix manages.

### Ensure Server Live (composite operation)

```rust
async fn ensure_live(&self, conv: &ConversationId) -> Result<Arc<RwLock<TmuxServer>>, TmuxError> {
    let server_arc = self.inner.entry(conv.clone())
        .or_insert_with(|| Arc::new(RwLock::new(TmuxServer::new(conv, &self.socket_dir))))
        .clone();

    let probe_result = probe(&server_arc.read().await.socket_path).await;
    let mut server = server_arc.write().await;
    match probe_result {
        ProbeResult::Live => {
            server.state = ServerState::Live;
        }
        ProbeResult::NoSocket => {
            spawn_session(&server.socket_path).await?;
            server.state = ServerState::Live;
        }
        ProbeResult::DeadSocket => {
            // System reboot recovery: socket file lingers but server is gone.
            // Silent recreate — no breadcrumb (the original draft's
            // send-keys breadcrumb was unsafe).
            tokio::fs::remove_file(&server.socket_path).await.ok();
            spawn_session(&server.socket_path).await?;
            server.state = ServerState::Live;
        }
    }
    drop(server);
    Ok(server_arc)
}

async fn spawn_session(socket_path: &Path) -> Result<(), TmuxError> {
    let status = tokio::process::Command::new("tmux")
        .args([
            "-S", &socket_path.to_string_lossy(),
            "new-session", "-d", "-s", "main",
        ])
        .env_remove("TMUX")  // see "TMUX env handling" below
        .status()
        .await?;
    if !status.success() {
        return Err(TmuxError::SpawnFailed);
    }
    Ok(())
}
```

### TMUX env handling

Phoenix MUST `env_remove("TMUX")` on every tmux subprocess invocation
(spawn_session, the tool dispatch, and the in-app terminal's `tmux
attach` exec). If the user launches Phoenix from inside an outer tmux
session, the inherited `TMUX` env var causes the inner tmux to refuse
to nest by default ("sessions should be nested with care; unset $TMUX
to force"). Stripping `TMUX` from the subprocess environment removes
the nesting check; Phoenix's own `-S` socket isolation is what keeps
the conversation's server distinct from the outer one.

The probe-and-act sequence runs at every operation; it is cheap (one
short-lived process spawn) and the only reliable way to detect both
post-Phoenix-restart (probe → live, no in-memory entry needed) and
post-system-reboot (probe → dead_socket, recreate).

## Tool Dispatch (REQ-TMUX-003, REQ-TMUX-010)

```rust
pub struct TmuxTool;

#[async_trait]
impl Tool for TmuxTool {
    fn name(&self) -> &str { "tmux" }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: TmuxInput = serde_json::from_value(input)?;

        let registry_arc = ctx.tmux().await?;
        let registry = registry_arc.read().await;
        if !registry.binary_available() {
            return ToolOutput::error("tmux_binary_unavailable",
                "the tmux binary is not installed on this host");
        }
        drop(registry);

        let server_arc = match TmuxRegistry::ensure_live_via(&ctx, &ctx.conversation_id).await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error("tmux_server_unavailable", e.to_string()),
        };
        let server = server_arc.read().await;
        let socket_path = server.socket_path.clone();
        drop(server);

        let wait_seconds = input.wait_seconds.unwrap_or(TMUX_TOOL_DEFAULT_WAIT_SECONDS);
        let mut full_args: Vec<String> = vec![
            "-S".into(),
            socket_path.to_string_lossy().into_owned(),
        ];
        full_args.extend(input.args);

        let mut cmd = tokio::process::Command::new("tmux");
        cmd.args(&full_args)
           .env_remove("TMUX")  // avoid outer-tmux nesting refusal
           .stdin(Stdio::null())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        let started = Instant::now();
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return ToolOutput::error("tmux_spawn_failed", e.to_string()),
        };

        tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {
                kill_child(&child).await;
                ToolOutput::structured("tmux", json!({
                    "status": "cancelled", "exit_code": null,
                    "duration_ms": started.elapsed().as_millis(),
                    "stdout": "", "stderr": "", "truncated": false,
                }))
            }
            _ = tokio::time::sleep(Duration::from_secs(wait_seconds)) => {
                kill_child(&child).await;
                let (so, se, truncated) = capture_truncated(child).await;
                ToolOutput::structured("tmux", json!({
                    "status": "timed_out", "exit_code": null,
                    "duration_ms": (wait_seconds * 1000) as u64,
                    "stdout": so, "stderr": se, "truncated": truncated,
                }))
            }
            output = child.wait_with_output() => {
                let output = output.unwrap();
                let (so, se, truncated) = truncate_pair(output.stdout, output.stderr);
                ToolOutput::structured("tmux", json!({
                    "status": "ok",
                    "exit_code": output.status.code(),
                    "duration_ms": started.elapsed().as_millis(),
                    "stdout": so, "stderr": se, "truncated": truncated,
                }))
            }
        }
    }
}
```

`truncate_pair` enforces `TMUX_OUTPUT_MAX_BYTES` over the combined size,
applying middle-truncation to whichever stream is over budget (or both if
both contribute).

## Terminal Attach Path (REQ-TMUX-004)

The terminal session-spawn path in `src/terminal/spawn.rs` consults the
registry before deciding what to exec inside the PTY:

```rust
async fn build_pty_exec_argv(ctx: &TerminalSpawnContext) -> Vec<CString> {
    let registry = &ctx.tmux_registry;
    if registry.binary_available() {
        let server_arc = registry.ensure_live(&ctx.conversation_id).await
            .expect("tmux ensure_live in terminal spawn path");
        let server = server_arc.read().await;
        vec![
            CString::new("tmux").unwrap(),
            CString::new("-S").unwrap(),
            CString::new(server.socket_path.to_string_lossy().into_owned()).unwrap(),
            CString::new("attach").unwrap(),
            CString::new("-t").unwrap(),
            CString::new("main").unwrap(),
        ]
    } else {
        vec![
            CString::new(user_shell()).unwrap(),
            CString::new("-i").unwrap(),
        ]
    }
}
```

The PTY spawn flow (fork, setsid, TIOCSCTTY, dup2, execvp) is identical
between the two cases; only the argv differs.

The existing single-terminal-per-conversation constraint from
`specs/terminal/` REQ-TERM-003 applies on both paths. Multi-attach via
tmux's native protocol is deferred (see the `TmuxMultiAttach` deferred
entry in `tmux-integration.allium`).

### No Stale-Recovery Breadcrumb

The earlier draft of this design rendered a one-line breadcrumb (`[phoenix]
previous tmux session lost at <ts>`) into the recovered pane after
stale-socket detection. The panel review surfaced that the proposed
mechanism (`tmux send-keys -l "<text>"`) writes to the slave PTY's stdin —
not to a display layer — so the breadcrumb would be interpreted as input
by whatever process happened to be running in the pane: typed into vim,
into a REPL, into a custom shell with command aliases.

Silent unlink-and-recreate is the safe v1 behaviour. The user discovers
the loss the next time they look at their windows; the pre-reboot pane
state is gone the same way it would be for any process killed by the
kernel reboot. If we later want to surface the loss, the right surface is
`tmux display-message` (status bar) or an audit-log-style entry, not
pane-input injection. This is captured as `TmuxStaleRecoveryNotification`
in the deferred section.

## Hard-Delete Cascade (REQ-TMUX-007)

Wired into the bedrock conversation-hard-delete handler:

```rust
async fn cascade_tmux_on_delete(registry: &TmuxRegistry, conv: &ConversationId) {
    let Some((_, server_arc)) = registry.inner.remove(conv) else {
        return; // no entry — nothing to do
    };
    let server = server_arc.read().await;
    let socket_path = server.socket_path.clone();
    drop(server);

    let _ = tokio::process::Command::new("tmux")
        .args(["-S", &socket_path.to_string_lossy(), "kill-server"])
        .status().await;
    let _ = tokio::fs::remove_file(&socket_path).await;
}
```

Errors are intentionally swallowed: the post-condition is "registry empty,
socket file gone, server process gone." A failed `kill-server` because the
server was already dead is the desired state. A failed `remove_file`
because the file was already absent is the desired state. Logging at
`debug` for each suppressed error is fine.

This runs alongside the bash handle cascade (`specs/bash/` REQ-BASH-006)
in the same hard-delete handler.

> **Bedrock dependency:** the cascade requires bedrock to emit a
> `ConversationHardDeleted` event (or expose a cascade-orchestrator
> hook) that this spec — and `specs/bash/`, `specs/projects/` — can
> subscribe to. At the time of this revision, bedrock has neither
> directly. The cascade integration is gated on adding that hook.

## ToolContext Extension (REQ-TMUX-013)

```rust
#[derive(Clone)]
pub struct ToolContext {
    pub cancel: CancellationToken,
    pub conversation_id: String,
    pub working_dir: PathBuf,
    browser_sessions: Arc<BrowserSessionManager>,
    bash_handles: Arc<BashHandleRegistry>,
    tmux_registry: Arc<TmuxRegistry>,
}

impl ToolContext {
    pub async fn tmux(&self) -> Result<Arc<RwLock<TmuxServer>>, TmuxError> {
        self.tmux_registry.ensure_live(&self.conversation_id).await
    }
}
```

The accessor signature (`async + Result + Arc<RwLock<...>>`) matches the
existing `ctx.browser()` and the `ctx.bash_handles()` defined in
`specs/bash/`. All three per-conversation tool registries share one
shape.

The terminal spawn path uses `registry.ensure_live` directly rather than
`ctx.tmux()` because the terminal spawn context isn't a `ToolContext`
(it carries different state); both call sites bottom out in
`TmuxRegistry::ensure_live`.

## Binary Availability Detection

```rust
fn detect_tmux_binary() -> bool {
    which::which("tmux").is_ok()
}
```

Runs once at registry initialisation. Cached for the process lifetime; we
do not redetect mid-session because that introduces ambiguity about
whether existing servers still exist (they don't — their socket files are
in the same place, regardless of the binary's PATH presence).

If the user installs tmux mid-session, they'll see the new behaviour after
their next Phoenix restart. This is acceptable.

## Output Capture and Truncation

```rust
fn truncate_pair(stdout: Vec<u8>, stderr: Vec<u8>) -> (String, String, bool) {
    let total = stdout.len() + stderr.len();
    if total <= TMUX_OUTPUT_MAX_BYTES {
        return (
            String::from_utf8_lossy(&stdout).into_owned(),
            String::from_utf8_lossy(&stderr).into_owned(),
            false,
        );
    }

    let budget_each = TMUX_OUTPUT_MAX_BYTES / 2;
    let so = truncate_middle(stdout, budget_each);
    let se = truncate_middle(stderr, TMUX_OUTPUT_MAX_BYTES - so.len());

    (so, se, true)
}

fn truncate_middle(bytes: Vec<u8>, max_bytes: usize) -> String {
    if bytes.len() <= max_bytes {
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    let keep = TMUX_TRUNCATION_KEEP_BYTES.min(max_bytes / 2);
    let head = &bytes[..keep];
    let tail = &bytes[bytes.len() - keep..];
    format!(
        "{}\n[output truncated in middle: got {}, kept {}+{}]\n{}",
        String::from_utf8_lossy(head),
        bytes.len(), keep, keep,
        String::from_utf8_lossy(tail),
    )
}
```

## Testing Strategy

### Unit tests
- Socket path resolution for various conversation ID shapes.
- `truncate_pair` budget allocation under various stdout/stderr ratios.
- ServerState transitions on each ProbeResult variant.

### Integration tests
- First tmux operation on a fresh conversation: socket dir created, server
  spawned, response delivered.
- Operation after server is alive (Phoenix restart simulation): probe → live,
  no spawn.
- Operation with stale socket file (unlink the live server's `.sock` while
  it's running, then re-issue): probe → dead_socket, file unlinked, fresh
  server, no breadcrumb in the recovered pane.
- Conversation hard-delete: kill-server runs, file unlinked, registry entry
  gone. Subsequent probes return NoSocket.
- Tool call passes args correctly: `tmux(["new-window", "-d", "-n", "x",
  "sleep", "1"])` produces a window named `x`.
- Tool call with `-L` or `-S` in agent args: tmux rejects with usage error;
  Phoenix surfaces it via the response.
- Cancellation via ToolContext.cancel mid-call: subprocess killed,
  status="cancelled" returned.
- Output truncation when subprocess emits >128KB.
- Terminal attach path with tmux available: PTY exec is `tmux attach`.
- Terminal attach path without tmux: PTY exec is `$SHELL -i`.
- Single-attach constraint preserved on the tmux path: a second open-
  terminal request is rejected per the existing terminal spec's
  behaviour.

### Property tests
- For any ConversationId, `socket_path_for(socket_dir, id)` is a unique
  path and a stable function of (dir, id).
- For any TmuxServer in state Live, `probe(server.socket_path) == Live`
  immediately after `ensure_live` returns (no race window).
- After hard-delete cascade returns, `socket_path` does not exist on disk
  and the registry has no entry for that conversation.

## Migration

The terminal spec's existing 22 requirements are unchanged in behaviour;
the new tmux-attach path is layered on top via the spawn-path branching
in `src/terminal/spawn.rs`. The single-attach constraint
(REQ-TERM-003) is preserved on both paths — multi-attach is deferred and
does not require any change to the existing terminal spec.

The bash spec's REQ-BASH-009 description text gets a sentence pointing at
the `tmux` tool for TTY/persistence/interactive needs.

The bedrock state machine gains the cascade hook (the
`ConversationHardDeleted` event called out as a dependency above).
Without it, the hard-delete cascade for tmux (and bash, and projects)
cannot be wired up.

The `ts_rs` codegen will pick up the new tool's response type
(`TmuxToolResponse`) once it derives `TS`. Run `./dev.py codegen`; update
valibot schemas in `ui/src/sseSchemas.ts`. The UI may want a small
indicator when the tmux tool is invoked vs. bash, since the response
shapes differ.

## File Organization

```
src/tools/
├── tmux.rs             # TmuxTool dispatch (REQ-TMUX-003)
├── tmux/
│   ├── registry.rs     # TmuxRegistry, TmuxServer, ensure_live, hard-delete cascade
│   ├── probe.rs        # probe(), ProbeResult
│   └── invoke.rs       # subprocess spawn, capture, truncation
src/terminal/
├── spawn.rs            # build_pty_exec_argv branches on tmux availability
└── ...
```

The project's clippy convention requires `foo.rs + foo/` rather than
`foo/mod.rs`; the layout above complies.
