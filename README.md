# Phoenix IDE

Web based, self-hosted LLM-powered coding agent.

Forking encouraged, make your own personal IDE that matches _your_ workflow.

Self hosted means you can poke it when it breaks.
All data is stored in a single local sqlite db.

Server/client architecture from day 1 means you can run the API half on a remote
coding VM and access from anywhere!

## Quick Start - LLM Access

Supported Options:
- API keys -> `ANTHROPIC_API_KEY` and/or `OPENAI_API_KEY`
- Paid ChatGPT / Codex -> in-app login!
    - "We want people to be able to use Codex, and their ChatGPT subscription,
      wherever they like!" - [from OpenAI themselves](https://x.com/romainhuet/status/2038699202834841962)
    - Browser and device code supported.
- Custom provider-compatible endpoint -> `ANTHROPIC_BASE_URL` / `OPENAI_BASE_URL`

See the env var section for more details (or point your preferred coding agent
at the repo)

## Quick Start - Run it


```bash
# Start everything (backend build + frontend dev server)
./dev.py up

# Other lifecycle commands
./dev.py down        # stop services
./dev.py restart     # restart services
./dev.py status      # show running state
./dev.py check       # pre-commit checks (fmt, clippy, tests)

# Optional: run the dev backend over HTTPS with h2 ALPN enabled
./dev.py up --https
```


### Single-shot CLI

```bash
# Runs via uv — no manual dependency install needed
./phoenix-client.py -d /tmp "Create hello.txt with 'Hello World'"
./phoenix-client.py -c <conversation-slug> "Now modify it"
```

## Architecture

Rust backend serves the API and, in production, embeds the React frontend via `rust-embed`.
SQLite persists conversations and messages. A bedrock state machine drives the conversation
lifecycle (Idle → Processing → ToolExecuting → …). Tools are modular and LLM-invokable.
Multi-provider LLM support: the Anthropic and OpenAI APIs, a ChatGPT/Codex bridge, or provider-compatible base URLs.

## Philosophy

**State is visible, not inferred.** The UI reflects exact system state —
which tool is running, how many are queued, what retry attempt you're on,
whether your message was sent or is still in the local queue. Nothing is
hidden behind a spinner.

**Deterministic core.** The conversation lifecycle is a pure state machine
(Elm architecture): same inputs always produce the same outputs. All I/O
is isolated in effect executors. State transitions are property-tested.

**Recoverable by default.** State is persisted to SQLite on every transition.
SSE reconnection replays missed events by sequence ID. Drafts survive
tab close. The server restarts to a known-good state.

**Subtle, not minimal.** The UI is information-dense — it communicates
clearly without wasting visual elements. Status is shown inline with
symbols and color, not buried in separate screens or modals. Progressive
disclosure: essentials visible by default, details on demand.

## Tools

| Tool | Description | Spec |
|------|-------------|------|
| bash | Shell command execution with wait windows, truncation, and handle-based background observation | [spec](specs/bash/executive.md) |
| patch | Structured file editing — create, modify, delete with fuzzy matching | [spec](specs/patch/executive.md) |
| read_file | Read a file, or a line range of one (any path the server process can access) | — |
| search | grep + glob over a directory tree, ripgrep-style (any path the server process can access) | — |
| keyword_search | Semantic code search using LLM-filtered results | [spec](specs/keyword_search/executive.md) |
| think | Reasoning scratchpad with zero side effects | [spec](specs/think/executive.md) |
| browser (`browser_*`) | Headless browser — navigate, eval JS, screenshot, click/type/keypress, resize, console logs | [spec](specs/browser-tool/executive.md) |
| read_image | Read and encode image files for vision models | — |
| tmux | Drive a persistent tmux session for long-lived / interactive processes | [spec](specs/tmux-integration/executive.md) |
| terminal_last_command / terminal_command_history | Inspect the in-app terminal's last command and history | [spec](specs/terminal/executive.md) |
| spawn_agents | Parallel task delegation to child agents | [spec](specs/subagents/executive.md) |
| ask_user_question | Ask the user a structured multiple-choice question mid-run | [spec](specs/ask-user-question/executive.md) |
| skill | Invoke a user-defined skill (SKILL.md instruction set) | [spec](specs/skills/executive.md) |
| propose_task | Propose a task file (task-authoring conversations only) | — |
| MCP tools | Tools exposed by configured MCP servers, registered dynamically at startup | — |

## Production Deployment

Designed to run as a background service: a single static binary that serves the HTTP API with the React UI embedded.

```bash
./dev.py prod deploy   # build release + deploy (launchd on macOS; systemd, or daemon mode if no systemd, on Linux)
./dev.py prod status   # check running production instance
./dev.py prod stop     # stop production instance
```

### Optional HTTPS Quick Start

TLS is opt-in. The lowest-toil internal-DNS flow is a local Phoenix private CA
that you trust once on your browser machine, then use to issue per-host leaf
certificates. The CA private key stays on the machine where you issue certs; the
remote host receives only its leaf cert and key.

```bash
# On the machine that owns the Phoenix CA, create/show the CA.
./dev.py tls ca

# Trust this CA cert once on the browser machine:
#   ~/.phoenix-ide/tls/phoenix-local-ca.pem

# Issue a bundle for the hostname you will open in the browser.
./dev.py tls issue phoenix-host.internal

# Copy only the bundle to the remote host.
scp ~/.phoenix-ide/tls-bundles/phoenix-host.internal.tar.gz ssh-host:~/

# On the remote host, from its phoenix-ide repo checkout:
./dev.py tls install ~/phoenix-host.internal.tar.gz
./dev.py prod deploy
```

After install, `./dev.py prod deploy` reads `.phoenix-ide.env` and serves
`https://phoenix-host.internal:8031`. For local development, `./dev.py up
--https` uses the same default CA directory, keeps the dev UI on the Vite URL,
and proxies API requests to Phoenix over HTTPS.

### Publishing a Release

```bash
./scripts/tag-release.sh v0.2.0   # validates clean tree, creates annotated tag, pushes
```

Pushing a `v*` tag triggers CI (`.github/workflows/release.yml`) which builds a static
`x86_64-unknown-linux-musl` binary and publishes it as a GitHub Release asset. The stable
download URL is:

```
https://github.com/scottopell/phoenix-ide/releases/latest/download/phoenix_ide-x86_64-unknown-linux-musl
```

## Environment Variables

Everything Phoenix reads. The server reads its config from the environment at
startup; `./dev.py` and the production deploy paths populate most of these for
you (prod reads `.phoenix-ide.env` from the repo root of the checkout you deploy from).

### Core server

| Variable | Purpose | Default |
|----------|---------|---------|
| `PHOENIX_PORT` | HTTP(S) listen port | `8000` |
| `PHOENIX_DB_PATH` | SQLite database path. If the path contains `prod`, startup logs "production mode" | `$HOME/.phoenix-ide/phoenix.db` |
| `PHOENIX_PASSWORD` | Optional auth password (REQ-AUTH-001). Empty/unset disables password auth | — (disabled) |
| `RUST_LOG` | `tracing` filter (`info`, `debug`, `phoenix_ide=debug`, …) | env-default filter |
| `HOME` / `USERPROFILE` | Home dir — used for the default DB path, built-in-skill extraction, working-dir resolution, tmux sockets | OS home; `/tmp` fallback |

### LLM providers and backend-compatible endpoints

| Variable | Purpose | Default |
|----------|---------|---------|
| `ANTHROPIC_API_KEY` | Direct Anthropic API key | — |
| `ANTHROPIC_BASE_URL` | Override the Anthropic API base URL | — |
| `OPENAI_API_KEY` | Direct OpenAI API key | — |
| `OPENAI_BASE_URL` | Legacy OpenAI endpoint override; prefer the API-format-specific overrides below | — |
| `OPENAI_RESPONSES_BASE_URL` | Override the OpenAI Responses endpoint | — |
| `OPENAI_CHAT_COMPLETIONS_BASE_URL` | Override the OpenAI Chat Completions endpoint | — |
| `DEFAULT_MODEL` | Preferred default model ID (used only if it actually registers) | first registered model |
| `PHOENIX_LLM_MODELS` | Inline JSON array of additional model specs to add to the built-in registry | — |
| `LLM_API_KEY_HELPER` | Command that prints a fresh API key/token on stdout (e.g. `claude` OAuth helper) | — |
| `LLM_API_KEY_HELPER_TTL_MS` | How long a helper-produced credential is cached | `7200000` (2 h) |
| `LLM_CUSTOM_HEADERS` | Extra request headers — newline-separated `Key: value` (literal `\n` accepted); a `provider` header is auto-injected | — |
| `LLM_REQUEST_TAGS` | Comma-separated `key=value` request tags | — |
| `LLM_AUTH_HEADER` | `bearer` → send the key as `Authorization: Bearer …`; anything else → provider's native API-key header | api-key style |
| `OPENAI_USE_CODEX_AUTH` | `1`/`true`/`yes`/`on` → route OpenAI models through ChatGPT/Codex credentials instead of `OPENAI_API_KEY` | off |
| `CODEX_HOME` | Where the Codex CLI keeps `auth.json` (read when the ChatGPT bridge is active) | `$HOME/.codex` |
| `PHOENIX_ENABLE_MOCK_MODEL` | `1` → register the deterministic mock provider (testing only) | off |

If none of `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `LLM_API_KEY_HELPER`,
or a ChatGPT/Codex credential is present, the server starts with no models and
logs a warning.

`PHOENIX_LLM_MODELS` is additive. It does not override built-in model IDs; a
configured duplicate ID is ignored and Phoenix logs a warning while keeping the
built-in definition. The value is parsed at startup, and invalid JSON or invalid
fields are logged and ignored without removing built-in models. Each configured
model object has this shape:

```json
[
  {
    "id": "provider-compatible/model-id",
    "api_name": "optional-wire-model-name",
    "backend": "anthropic",
    "description": "Human-readable picker text",
    "context_window": 200000,
    "max_output_tokens": 16384,
    "recommended": false,
    "supports_tool_search": false
  }
]
```

`api_name` may be omitted; when absent Phoenix sends `id` as the wire model
name. `backend` selects both route/auth family and wire protocol. Supported
values are `anthropic` (Anthropic Messages-compatible), `openai_responses`
(OpenAI Responses-compatible), and `openai_chat_completions` (OpenAI Chat
Completions-compatible). `max_output_tokens` may be omitted; when present it
must be nonzero and lower than `context_window`.

Example Anthropic-compatible provider POC:

```env
ANTHROPIC_API_KEY=provider-api-key
ANTHROPIC_BASE_URL=https://provider.example/v1/messages
LLM_CUSTOM_HEADERS=source: my-provider-poc
DEFAULT_MODEL=example/provider-model
PHOENIX_LLM_MODELS=[{"id":"example/provider-model","backend":"anthropic","description":"Example Anthropic-compatible POC","context_window":200000,"recommended":false,"supports_tool_search":false}]
```

Example OpenAI-compatible Chat Completions gateway POC:

```env
OPENAI_API_KEY=provider-api-key
OPENAI_CHAT_COMPLETIONS_BASE_URL=https://provider.example/v1/chat/completions
DEFAULT_MODEL=example/chat-model
PHOENIX_LLM_MODELS=[{"id":"example/chat-model","backend":"openai_chat_completions","description":"Example Chat Completions-compatible POC","context_window":200000,"max_output_tokens":16384,"recommended":false,"supports_tool_search":false}]
```

### TLS

| Variable | Purpose | Default |
|----------|---------|---------|
| `PHOENIX_TLS` | HTTPS mode: `auto`/`on`/`true`/`1`, `manual`, or `off`/`none`/`false`/`0` | `off` |
| `PHOENIX_TLS_HOSTS` | Comma-separated extra DNS/IP SANs for `PHOENIX_TLS=auto` | `localhost,127.0.0.1,::1` |
| `PHOENIX_TLS_DIR` | Managed local-CA + auto-issued leaf certificate directory | parent of `PHOENIX_DB_PATH` + `/tls` |
| `PHOENIX_TLS_CERT_PATH` | Manual TLS certificate PEM path; together with the key path, enables manual TLS even if `PHOENIX_TLS` is unset | — |
| `PHOENIX_TLS_KEY_PATH` | Manual TLS private-key PEM path; required alongside the cert path | — |

TLS is opt-in. `PHOENIX_TLS=auto` creates a private Phoenix CA in
`PHOENIX_TLS_DIR` if one is not already present, then rotates the server leaf
certificate on startup. `PHOENIX_TLS=manual` serves the cert/key paths exactly as
configured; this is what `./dev.py tls install` writes for remote production
hosts. See [TLS.md](TLS.md) for the complete trust and deployment workflow.

### Tools and runtime

| Variable | Purpose | Default |
|----------|---------|---------|
| `PHOENIX_DATA_DIR` | Base dir for runtime data — currently the per-conversation tmux socket dir (`$PHOENIX_DATA_DIR/tmux-sockets/`) | `$HOME/.phoenix-ide` |
| `PHOENIX_CHROME_EXECUTABLE` | Explicit Chrome/Chromium path for the `browser` tool; falls back to auto-detection | auto-detect |
| `PHOENIX_PARENT_TOOL_CYCLE_CAP` | Max tool cycles a parent agent may run before yielding (non-negative int; `0` disables the cap) | built-in default |
| `PATH` | Used to locate `tmux`, Chrome, and shell binaries; inherited into the in-app terminal | inherited |
| `SHELL` | Shell the in-app terminal launches | `/bin/bash` |

The in-app terminal is given a **deliberately minimal** environment (it never
inherits the server's env, to avoid leaking secrets): `TERM`, `COLORTERM`,
`HOME`, `USER`/`LOGNAME`, `SHELL`, `PATH`, `LANG`, plus shell-integration hints
(`TERM_PROGRAM=phoenix-ide`, `ITERM_SHELL_INTEGRATION_INSTALLED=Yes`). `USER`
falls back to `LOGNAME`.

### Zero-downtime restart (set by socket activation, not by you)

| Variable | Purpose |
|----------|---------|
| `LISTEN_FDS` / `LISTEN_PID` | systemd-style socket passing — the listening socket survives an in-place binary swap (`./dev.py restart`, `prod deploy`). Managed automatically; the binary clears them after adopting the fd. |
| launchd `Sockets` / `Listeners` | macOS launchd socket passing — `./dev.py prod deploy` installs an IPv4/v6 dual-stack listener named `Listeners`, and the binary adopts it with `launch_activate_socket`. |

### Tests only (`cargo test`)

| Variable | Purpose |
|----------|---------|
| `PHOENIX_SKIP_BROWSER_TESTS` | `1` → skip browser-tool tests (no Chrome available) |
| `PHOENIX_SKIP_NETWORK_TESTS` | `1` → skip tests that need outbound network |

### Dev/build tooling only (read by `./dev.py` / Vite, **not** the server)

| Variable | Purpose |
|----------|---------|
| `PHOENIX_PUBLIC_URL` | Display URL shown in `./dev.py prod status` / deploy output |
| `PHOENIX_VERSION` | Version string `./dev.py` bakes into the systemd/prod env (the server reports its own `CARGO_PKG_VERSION`) |
| `VITE_API_PORT` / `VITE_API_SCHEME` / `VITE_API_PROXY_SECURE` | How the Vite dev server proxies `/api` to Phoenix |

## API Endpoints

- `GET /api/conversations` - List all conversations
- `POST /api/conversations/new` - Create new conversation
- `GET /api/conversations/:id` - Get conversation details
- `POST /api/conversations/:id/messages` - Send a message
- `GET /api/conversations/:id/stream` - SSE stream for real-time updates

## Documentation

- `specs/` — Per-tool and subsystem specs using the [spEARS methodology](SPEARS.md)
- [LAUNCHD.md](LAUNCHD.md) — macOS launchd deployment and socket activation
- [TLS.md](TLS.md) — HTTPS, HTTP/2, private CA, and deployment workflow
- [AGENTS.md](AGENTS.md) — Agent architecture and conventions
