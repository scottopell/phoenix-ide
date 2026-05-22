# Add a pit-of-success `tmux_run` tool for shared inspectable commands

## Problem

Phoenix exposes a raw `tmux` tool against the conversation/worktree-isolated tmux socket. That is powerful, but it is not a pit of success for the most common agent use case: start a dev server or watcher such as `./dev.py up` in a shared tmux surface that the user can inspect later.

The raw tmux invocation that works is ceremony-heavy:

```bash
tmux new-window -d -n phoenix-dev \
  -c /path/to/conversation/file-root \
  bash -lc './dev.py up; code=$?; echo EXIT:$code; sleep 3600'
```

The agent should not need to know tmux cwd inheritance rules to run `./dev.py up` correctly.

Current footguns:

- The tmux socket isolates the server namespace, but does not enforce a default cwd for future `new-window` / `split-window` commands.
- Direct conversations have an immutable `cwd`; Work/Branch/managed Explore conversations are rooted at `conv_mode.worktree_path()`.
- Raw `tmux new-window` without `-c <file-root>` can start somewhere other than Phoenix's intended file root.
- Short-lived command failures can close the window before the agent can capture output.
- Compound command strings need shell wrapping (`bash -lc`) for exit-code handling and follow-up commands.

## Goal

Add a small, explicit, agent-facing `tmux_run` tool that starts a shell command inside Phoenix's shared tmux surface with Phoenix defaults:

- correct conversation file root
- shell wrapping
- visible exit marker
- pane remains inspectable after command exit
- user can later inspect/interact via the existing tmux surface

Keep the existing raw `tmux` tool for detailed tmux operations for now. Long-term, raw tmux may be deprecated, hidden from normal agents, or subsumed by another lower-level mechanism, but this task should not require that decision.

## Strawman tool design

Tool name: `tmux_run`

Intent-oriented description:

> Run a shell command in this conversation's shared tmux surface. Use this for dev servers, watchers, REPLs, and commands the user may want to inspect later. Phoenix chooses the correct project directory automatically and starts the command there. The pane stays available after exit and prints the exit code.
>
> Avoid exposing conversation-mode jargon in the agent-facing description. The implementation should use the typed conversation mode to choose the directory, but the agent only needs the simpler contract: "this runs in the same project/worktree I am working in."

### Input schema

Minimal v1:

```json
{
  "type": "object",
  "required": ["cmd"],
  "properties": {
    "cmd": {
      "type": "string",
      "description": "Shell command to run via bash -lc, e.g. ./dev.py up"
    },
    "name": {
      "type": "string",
      "description": "Optional tmux window name. If omitted, Phoenix derives a short stable name from the command."
    },
    "keep_open_on_exit": {
      "type": "boolean",
      "default": true,
      "description": "Keep the pane inspectable after the command exits. Defaults to true."
    }
  }
}
```

Readiness waiting is in v1 scope and must be modeled structurally. Do not add loose sibling fields like `wait_for?: string` plus `timeout_seconds?: number`; that permits nonsensical states such as an empty readiness string with a long timeout.

Model readiness as a tagged variant / discriminated union:

```json
{
  "readiness": {
    "mode": "return_immediately"
  }
}
```

or:

```json
{
  "readiness": {
    "mode": "wait_for_text",
    "text": "Phoenix backend listening",
    "timeout_seconds": 120
  }
}
```

Rules:

- If omitted, `readiness` defaults to `return_immediately`.
- `return_immediately` has no timeout field.
- `wait_for_text.text` must be non-empty after trimming.
- `wait_for_text.timeout_seconds` is required and bounded.
- The response status distinguishes `started` from `ready` from `readiness_timed_out`.

### Output shape

Strawman response:

```json
{
  "status": "started" | "ready" | "exited" | "readiness_timed_out",
  "window_name": "phoenix-dev",
  "cwd": "/absolute/file/root",
  "command": "./dev.py up",
  "exit_code": null,
  "captured_output": {
    "stdout": "recent startup output or readiness evidence",
    "stderr": "",
    "truncated": false
  }
}
```

`window_name` is the tmux window name created by this tool. That is enough for v1 because existing raw tmux commands already accept `-t <window-name>` when the agent needs to inspect later, e.g. `capture-pane -p -t phoenix-dev -S -2000`.

Do not add speculative inspect-handle objects or precomputed `capture_command` fields until another tool consumes that structure. Keep the response simple and aligned with existing raw tmux usage.

`captured_output.truncated` belongs with the captured output, not at top level, because truncation only describes the bounded snippet returned by this tool call. It does not mean the tmux pane itself is truncated; the pane/window remains inspectable through raw tmux using `window_name`.

## Execution semantics

`tmux_run({ cmd, name, keep_open_on_exit })` should:

1. Resolve the conversation's effective file root:
   - Work / Branch / managed Explore: `conv_mode.worktree_path()` / `ToolContext.worktree_path`
   - Direct: immutable `ToolContext.working_dir`
2. Ensure the conversation/worktree tmux server is live using the existing registry/socket logic.
3. Create a new tmux window in that server with `-c <file-root>`.
4. Run the command through `bash -lc`.
5. Emit a standardized exit marker.
6. Keep the pane inspectable after exit by default.

Initial wrapper can be simple and explicit:

```bash
bash -lc '<cmd>; code=$?; echo; echo "[phoenix] process exited with code $code"; exec ${SHELL:-/bin/bash} -i'
```

Alternative implementation can use tmux `remain-on-exit` if testing proves it is cleaner, but do not block the v1 helper on that choice.

## Relationship to raw `tmux`

Update the raw `tmux` description to steer agents:

- Use `tmux_run` for starting dev servers, watchers, REPLs, or inspectable commands.
- Use raw `tmux` for detailed tmux operations: `capture-pane`, `send-keys`, `list-windows`, `kill-window`, etc.
- Raw `tmux` is pass-through except for Phoenix's socket/config injection; it does not enforce cwd for newly-created windows/panes.

Do not silently rewrite raw `tmux new-window` in this task. Keep raw raw.

## Acceptance criteria

- [ ] `tmux_run({"cmd":"./dev.py up", "name":"phoenix-dev"})` starts a tmux window named `phoenix-dev` in the conversation's effective file root.
- [ ] Direct conversations use their immutable `cwd` as the file root.
- [ ] The agent-facing tool description says Phoenix starts the command in the current project/worktree automatically; detailed Direct/Work/Branch/Explore terminology stays in implementation docs/tests, not the prompt-facing happy path.
- [ ] Readiness waiting is represented as a tagged shape (e.g. `return_immediately` vs `wait_for_text`) so empty wait strings and orphaned timeouts are structurally invalid.
- [ ] A command that exits quickly leaves inspectable output in the tmux pane/window.
- [ ] The pane/window includes a visible standardized exit marker with the exit code.
- [ ] The tool returns the tmux `window_name` so the agent can call raw `tmux capture-pane -p -t <window_name> -S -2000` later.
- [ ] Truncation is scoped to the returned captured-output snippet and cannot be confused with tmux pane scrollback availability.
- [ ] Raw `tmux` remains available and pass-through.
- [ ] Raw `tmux` tool description points agents to `tmux_run` for the dev-server/watch-command use case.
- [ ] Tests cover cwd selection and quick-failure inspectability.

## Notes / open design choices

- Tool name is intentionally `tmux_run`, not `terminal_run`: tmux is a real Phoenix surface and agents should understand that the output is inspectable via tmux commands.
- Readiness waiting is in v1 scope and must use a tagged readiness variant with non-empty text and bounded timeout.
- Real tombstoned output/logging should be deferred unless the simple keep-open behavior is insufficient.
- Raw `tmux` might eventually be hidden, deprecated, or replaced by intercepting `tmux` invocations through another command tool, but that is out of scope for this task.
