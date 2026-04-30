# Tmux Integration — Design Document

## Overview

The tmux integration is two cooperating pieces:

1. The agent-facing `tmux` tool — a pass-through that prepends
   `-L <conv-sock>` and forwards the rest of the args to the tmux binary.
2. The terminal feature's tmux-attach behaviour — when the tmux binary is
   available, the in-app terminal's PTY runs `tmux attach` against the
   conversation's socket instead of the user's `$SHELL` directly.

Both share a per-conversation tmux server registry that handles socket-path
resolution, lazy spawn, stale-socket detection, and the conversation-hard-
delete cascade. The registry is reachable via `ToolContext.tmux()` for the
agent tool and consumed directly by the terminal session-spawn path.

## Tool Surface (REQ-TMUX-003, REQ-TMUX-010, REQ-TMUX-013)

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

Note: Persistence is across Phoenix restart only, not system reboot. After a
host reboot, this server's state is lost; you'll see a "[phoenix] previous
tmux session lost at <ts>" breadcrumb in the next terminal you open.
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
    inner: Arc<DashMap<ConversationId, Arc<TmuxServer>>>,
    socket_dir: PathBuf,    // ~/.phoenix-ide/tmux-sockets/
    binary_available: bool, // discovered once at startup
}

pub struct TmuxServer {
    conversation_id: ConversationId,
    socket_path: PathBuf,
    state: ArcSwap<ServerState>,
}

pub enum ServerState {
    NotProbed,          // initial, before any operation
    Live { stale_breadcrumb_pending: bool },
    Gone,               // post-hard-delete; entry is dropped from registry
}
```

`ArcSwap` lets in-flight readers (e.g., terminal session deciding to attach
versus spawn) observe atomic state transitions without holding the registry
lock.

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

### Probe (REQ-TMUX-006, REQ-TMUX-007)

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
        .args(["-L", socket_path_label(socket_path), "ls"])
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

`socket_path_label` extracts the basename without `.sock` for use as the
`-L` argument: `tmux -L conv-42 ls` (tmux's `-L` takes a label, not a full
path; it joins the label against `$TMUX_TMPDIR` or `/tmp/tmux-<uid>`).

> **Implementation note — socket path vs label:** Tmux's `-L` is a *label*,
> not a path. By default it resolves under `/tmp/tmux-<uid>/` (or
> `$TMUX_TMPDIR` if set). To pin sockets at
> `~/.phoenix-ide/tmux-sockets/`, Phoenix sets `TMUX_TMPDIR=
> ~/.phoenix-ide/tmux-sockets/` in the environment of every tmux invocation
> and uses simple labels like `conv-42`. The on-disk socket path becomes
> `~/.phoenix-ide/tmux-sockets/conv-42`. (Confirm during implementation: if
> `TMUX_TMPDIR` semantics fight with this layout, the alternative is to
> use `-S <absolute-path>` instead, accepting that the agent's stray `-L`
> arguments would then conflict with `-S` rather than getting overridden.
> The Allium spec language permits either; this design picks `-L` +
> `TMUX_TMPDIR` as the cleaner front-running.)

### Ensure Server Live (composite operation)

```rust
async fn ensure_live(&self, conv: &ConversationId) -> Result<Arc<TmuxServer>, TmuxError> {
    let server = self.inner.entry(conv.clone())
        .or_insert_with(|| Arc::new(TmuxServer::new(conv, &self.socket_dir)))
        .clone();

    let probe_result = probe(&server.socket_path).await;
    match (server.state.load().as_ref(), probe_result) {
        (_, ProbeResult::Live) => {
            // Probe confirms; mark live (no breadcrumb if it was already live).
            let breadcrumb_pending = matches!(server.state.load().as_ref(),
                                              ServerState::Live { stale_breadcrumb_pending: true });
            server.state.store(Arc::new(ServerState::Live { stale_breadcrumb_pending: breadcrumb_pending }));
            Ok(server)
        }
        (_, ProbeResult::NoSocket) => {
            spawn_session(&server.socket_path).await?;
            server.state.store(Arc::new(ServerState::Live { stale_breadcrumb_pending: false }));
            Ok(server)
        }
        (_, ProbeResult::DeadSocket) => {
            tokio::fs::remove_file(&server.socket_path).await.ok();
            spawn_session(&server.socket_path).await?;
            server.state.store(Arc::new(ServerState::Live { stale_breadcrumb_pending: true }));
            Ok(server)
        }
    }
}

async fn spawn_session(socket_path: &Path) -> Result<(), TmuxError> {
    let status = tokio::process::Command::new("tmux")
        .env("TMUX_TMPDIR", socket_path.parent().unwrap())
        .args(["-L", socket_label(socket_path), "new-session", "-d", "-s", "main"])
        .status()
        .await?;
    if !status.success() {
        return Err(TmuxError::SpawnFailed);
    }
    Ok(())
}
```

The probe-and-act sequence runs at every operation; it is cheap (one
short-lived process spawn) and the only reliable way to detect both
post-Phoenix-restart (probe → live, no in-memory entry) and post-system-
reboot (probe → dead_socket).

## Tool Dispatch (REQ-TMUX-003, REQ-TMUX-011)

```rust
pub struct TmuxTool;

#[async_trait]
impl Tool for TmuxTool {
    fn name(&self) -> &str { "tmux" }

    async fn run(&self, input: Value, ctx: ToolContext) -> ToolOutput {
        let input: TmuxInput = serde_json::from_value(input)?;

        let registry = ctx.tmux();
        if !registry.binary_available() {
            return ToolOutput::error("tmux_binary_unavailable",
                "the tmux binary is not installed on this host");
        }

        let server = match registry.ensure_live(&ctx.conversation_id).await {
            Ok(s) => s,
            Err(e) => return ToolOutput::error("tmux_server_unavailable", e.to_string()),
        };

        let wait_seconds = input.wait_seconds.unwrap_or(TMUX_TOOL_TIMEOUT_SECONDS);
        let mut full_args: Vec<String> = vec![
            "-L".into(),
            socket_label(&server.socket_path).into(),
        ];
        full_args.extend(input.args);

        let mut cmd = tokio::process::Command::new("tmux");
        cmd.env("TMUX_TMPDIR", server.socket_path.parent().unwrap())
           .args(&full_args)
           .stdin(Stdio::null())
           .stdout(Stdio::piped())
           .stderr(Stdio::piped());

        let started = Instant::now();
        let child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return ToolOutput::error("tmux_spawn_failed", e.to_string()),
        };

        let res = tokio::select! {
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
                let (so, se, truncated) = capture_truncated(&mut child).await;
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
        };
        res
    }
}
```

`truncate_pair` enforces `TMUX_OUTPUT_MAX_BYTES` over the combined size,
applying middle-truncation to whichever stream is over budget (or both if
both contribute).

## Terminal Attach Path (REQ-TMUX-004, REQ-TMUX-005)

The terminal session-spawn path in `src/terminal/spawn.rs` consults the
registry before deciding what to exec inside the PTY:

```rust
async fn build_pty_exec_argv(ctx: &TerminalSpawnContext) -> Vec<CString> {
    let registry = &ctx.tmux_registry;
    if registry.binary_available() {
        let server = registry.ensure_live(&ctx.conversation_id).await
            .expect("tmux ensure_live in terminal spawn path");
        // attach with literal CR after to flush any pending breadcrumb send-keys
        vec![
            CString::new("tmux").unwrap(),
            CString::new("-L").unwrap(),
            CString::new(socket_label(&server.socket_path)).unwrap(),
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

### Breadcrumb Rendering (REQ-TMUX-007)

After `ensure_live` returns with `stale_breadcrumb_pending = true`, the
terminal-spawn path injects a breadcrumb before issuing attach:

```rust
async fn maybe_render_breadcrumb(server: &TmuxServer) -> Result<(), TmuxError> {
    let mut state_arc = server.state.load_full();
    let pending = matches!(state_arc.as_ref(),
                           ServerState::Live { stale_breadcrumb_pending: true });
    if !pending { return Ok(()); }

    let line = format!("[phoenix] previous tmux session lost at {}",
                       human_ts(SystemTime::now()));
    // Write into the main pane via send-keys -l (literal, not interpreted).
    let mut cmd = tokio::process::Command::new("tmux");
    cmd.env("TMUX_TMPDIR", server.socket_path.parent().unwrap())
       .args(["-L", socket_label(&server.socket_path),
              "send-keys", "-t", "main", "-l", &line])
       .status().await?;

    // CR after, so the breadcrumb is on its own line and the pane scrolls.
    let mut cmd = tokio::process::Command::new("tmux");
    cmd.env("TMUX_TMPDIR", server.socket_path.parent().unwrap())
       .args(["-L", socket_label(&server.socket_path),
              "send-keys", "-t", "main", "Enter"])
       .status().await?;

    server.state.store(Arc::new(ServerState::Live { stale_breadcrumb_pending: false }));
    Ok(())
}
```

The breadcrumb is a once-per-recovery event. After it's written into the
pane, every subsequent attach (multi-attach scenarios) sees it as part of
pane history.

### Multi-Attach (REQ-TMUX-005)

The terminal spec's "exactly one terminal per conversation" constraint
applies only to the direct-PTY fallback. On the tmux path, each new
terminal connection spawns its own PTY child running `tmux attach`; tmux's
multi-client protocol handles synchronisation. The terminal session
registry must distinguish the two cases:

```rust
pub struct TerminalRegistry {
    sessions: HashMap<ConversationId, TerminalSessionGroup>,
}

pub enum TerminalSessionGroup {
    DirectPty { single: TerminalHandle },     // existing model
    TmuxAttach { clients: Vec<TerminalHandle> },  // multi-attach
}
```

On attach request, the tmux-path branch always succeeds (no 409).
DuplicateConnectionReclaimsSession (terminal/REQ-TERM-003) governs the
direct-PTY case unchanged.

## Hard-Delete Cascade (REQ-TMUX-008)

Wired into the bedrock conversation-hard-delete handler:

```rust
async fn cascade_tmux_on_delete(registry: &TmuxRegistry, conv: &ConversationId) {
    let Some(server) = registry.inner.remove(conv).map(|(_, v)| v) else {
        return; // no entry — nothing to do
    };
    let _ = tokio::process::Command::new("tmux")
        .env("TMUX_TMPDIR", server.socket_path.parent().unwrap())
        .args(["-L", socket_label(&server.socket_path), "kill-server"])
        .status().await;
    let _ = tokio::fs::remove_file(&server.socket_path).await;
}
```

Errors are intentionally swallowed: the post-condition is "registry empty,
socket file gone, server process gone." A failed `kill-server` because the
server was already dead is the desired state. A failed `remove_file`
because the file was already absent is the desired state. Logging at
`debug` for each suppressed error is fine.

This runs alongside the bash handle cascade (`specs/bash/` REQ-BASH-006) in
the same hard-delete transaction.

## ToolContext Extension (REQ-TMUX-014)

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
    pub fn tmux(&self) -> &TmuxRegistry {
        &self.tmux_registry
    }
}
```

The terminal spawn path receives the same registry directly via the
terminal session's spawn context.

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

    // Allocate the budget proportionally; truncate each stream's middle.
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
- Socket path resolution for various conversation ID shapes (UUIDs, slugs,
  numeric).
- `truncate_pair` budget allocation under various stdout/stderr ratios.
- `socket_label` extraction from an absolute path.
- ServerState transitions on each ProbeResult variant.

### Integration tests
- First tmux operation on a fresh conversation: socket dir created, server
  spawned, response delivered.
- Operation after server is alive (Phoenix restart simulation): probe → live,
  no spawn.
- Operation with stale socket file (unlink the live server's `.sock` while
  it's running, then re-issue): probe → dead_socket, file unlinked, fresh
  server, breadcrumb pending. Check breadcrumb appears in the next attach.
- Conversation hard-delete: kill-server runs, file unlinked, registry entry
  gone. Subsequent probes return NoSocket.
- Tool call passes args correctly: `tmux(["new-window", "-d", "-n", "x",
  "sleep", "1"])` produces a window named `x`.
- Tool call with `-L` in agent args: the agent's `-L` is ignored (Phoenix's
  -L wins by argument order); window operations still target the right
  server.
- Cancellation via ToolContext.cancel mid-call: subprocess killed,
  status="cancelled" returned.
- Output truncation when subprocess emits >128KB.
- Terminal attach path with tmux available: PTY exec is `tmux attach`;
  multi-attach succeeds.
- Terminal attach path without tmux: PTY exec is `$SHELL -i`; second
  attach is rejected per existing terminal-spec behaviour.

### Property tests
- For any ConversationId, `socket_path_for(socket_dir, id)` is a unique
  path and a stable function of (dir, id).
- For any TmuxServer in state Live, `probe(server.socket_path) == Live`
  immediately after `ensure_live` returns (no race window).
- After hard-delete cascade returns, `socket_path` does not exist on disk
  and the registry has no entry for that conversation.

## Migration

The terminal spec gains REQ-TERM-015 and REQ-TERM-016 (cross-referenced
to this spec); the existing 14 terminal requirements are unchanged in
behaviour but the "exactly one terminal per conversation" wording is
weakened to "exactly one direct-PTY terminal" so multi-attach on the tmux
path is consistent with the spec.

The bash spec's REQ-BASH-009 description text gets a sentence pointing at
the `tmux` tool for TTY/persistence/interactive needs.

The bedrock hard-delete cascade gains a new step: in addition to the
existing tear-downs, run `cascade_tmux_on_delete` for the deleted
conversation. The cascade ordering is: kill bash handles → kill tmux
server → remove conversation row.

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
│   ├── invoke.rs       # subprocess spawn, capture, truncation
│   └── breadcrumb.rs   # stale-recovery breadcrumb rendering
src/terminal/
├── spawn.rs            # build_pty_exec_argv branches on tmux availability
└── ...
```
