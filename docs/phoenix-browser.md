# Phoenix browser for coding agents

Build and install from the workspace (Rust 1.95 or later):

```sh
cargo install --path crates/phoenix-tools --bin phoenix-browser --locked
phoenix-browser --help
```

The executable runs independently of the Phoenix server on macOS and Linux. It uses system Chrome, `PHOENIX_CHROME_EXECUTABLE` when set, or downloads a compatible Chromium into Phoenix's cache. The first browser launch can take longer while downloading.

## Bind the session

For a coding harness, set one name for the agent's lifetime:

```sh
export PHOENIX_BROWSER_SESSION="my-agent-unique-id"
```

An explicit `--session NAME` overrides the environment. Without either, commands share a session selected by the canonical Git worktree root, or current directory outside Git. Subdirectories share the worktree browser; linked worktrees have different browsers. Agents sharing a worktree should bind unique names.

## Inspect and interact

```sh
phoenix-browser --json open http://localhost:3000
phoenix-browser snapshot --interactive
phoenix-browser click '@COPY-THE-REF-FROM-THE-SNAPSHOT'
phoenix-browser type '#search' 'query' --clear
phoenix-browser key Enter
phoenix-browser wait '#results'
phoenix-browser console
phoenix-browser screenshot
printf '%s\n' 'document.title' | phoenix-browser --json eval --stdin
```

`open` returns URL, title, an interactive accessibility snapshot, and console errors captured for that navigation. Snapshot failure appears separately from navigation success. Large snapshots spill to a readable artifact; the returned refs work after reading it. Snapshot refs identify exact nodes and expire on a new snapshot attempt, document replacement, or browser replacement. Refresh the snapshot when a ref is stale. CSS selectors also work with click/type.

Navigation observation failures appear separately in `observation_errors`; console capture failure has its own `console_capture_error`. Reference input fails visibly when the browser cannot verify the native event destination, including a closed shadow root created without the early input guard.

`--json` returns `session`, daemon `lifetime`, browser `browser_lifetime`, and a `status` tag: `success` with `result`, or `error` with `error.code` and `error.message`. Tool results include `text`, structured `data`, and explicit `artifacts`. Tool errors preserve structured failure data in `error.details` and any `error.artifacts`. Failed commands exit 1; invalid input exits 2 before looking up browser state. Human output prints concise operation text; JSON is the harness interface.

Use `schema [COMMAND]` for the full operation parameters. Each command accepts `--input '{...}'`; this is especially useful for profiling. `--timeout 15s` controls operations that accept a timeout.

Use `--` before positional text that matches an option, for example `phoenix-browser type '#field' -- '--json'`.

## Profile

```sh
phoenix-browser profile help
phoenix-browser profile cpu_start
phoenix-browser eval 'doWork()'
phoenix-browser profile cpu_stop
phoenix-browser profile cpu_summary --input '{"path":"/absolute/path/from/cpu_stop.cpuprofile"}'
phoenix-browser --json profile run_scenario --input '{"runs":5,"warmup":1,"steps":[{"kind":"eval","expression":"document.body.offsetHeight"}]}'
```

Use the schema and profiling help for scenario step shapes, readiness, resets, CPU throttling, heap captures, coverage, traces, and React diagnostics. Preserve raw samples and compute statistics separately. CPU captures and other active profiling state persist between invocations. Profiling help and saved CPU-profile analysis run locally without a browser.

## Lifecycle and recovery

```sh
phoenix-browser status
phoenix-browser close
phoenix-browser --idle-timeout 600 open http://localhost:3000
```

Navigation starts a daemon and fresh transient Chrome profile lazily. The default idle timeout is 1800 seconds; `--idle-timeout SECONDS` overrides `PHOENIX_BROWSER_IDLE_TIMEOUT` when starting a daemon. An existing daemon retains its configuration, shown by status. Idle configuration is read only when starting a daemon; a malformed setting does not block status, close, schema, or file-only profiling commands. Active operations can exceed the idle timeout. Explicit close and orderly signals interrupt admitted operations with `operation_cancelled`; page side effects may already have occurred. Browser initialization settles under ownership before cancellation and cleanup, so shutdown cannot abandon an unregistered Chrome. After close or idle expiry, interaction/eval/profile commands report `state_lost`; `open URL` intentionally starts fresh. Status and close report absence without launching anything.

Commands in a session are serialized. If a connection fails after a request may have arrived, the CLI reports `transport_uncertain` and never retries it automatically. Inspect page state before repeating a side effect.

If cleanup fails, the daemon retains session ownership and rejects new browser work. Inspect `status`, repair the reported resource, and retry `close`; a replacement browser cannot start under that session until cleanup succeeds.

Control files and sockets live in a private `browser-cli` directory under Phoenix's data directory. `PHOENIX_BROWSER_RUNTIME_DIR` can select a shorter absolute private directory if a Unix socket path is too long. Browser profiles and accessibility snapshot artifacts use Phoenix's configured scratch root; each command returns its artifact locations. Snapshot artifacts are retired by the next snapshot attempt, document replacement, or browser teardown. The daemon log path appears in startup errors. Close sessions before upgrading/replacing the executable; interoperability with older daemon versions and recovery after an ungraceful daemon death are not promised. No browser profile import or credential extraction is supported.
