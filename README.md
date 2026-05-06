# Phoenix IDE

LLM-powered coding agent. Rust backend, React frontend, self-hosted.

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

## Quick Start

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
Multi-provider LLM support routes through either the Anthropic API or an exe.dev gateway.

## Tools

| Tool | Description | Spec |
|------|-------------|------|
| bash | Shell command execution with timeout, truncation, background mode | [spec](specs/bash/executive.md) |
| patch | Structured file editing — create, modify, delete with fuzzy matching | [spec](specs/patch/executive.md) |
| keyword_search | Semantic code search using LLM-filtered results | [spec](specs/keyword_search/executive.md) |
| think | Reasoning scratchpad with zero side effects | [spec](specs/think/executive.md) |
| browser | Headless browser — navigate, eval JS, screenshot, console logs | [spec](specs/browser-tool/executive.md) |
| read_image | Read and encode image files for vision models | — |
| subagent | Parallel task delegation to child agents | — |

## Production Deployment

```bash
./dev.py prod deploy   # build release + deploy (auto-detects Linux native vs macOS+Lima)
./dev.py prod status   # check running production instance
./dev.py prod stop     # stop production instance

# Lima VM lifecycle (macOS only)
./dev.py lima create   # provision VM
./dev.py lima shell    # SSH into VM
./dev.py lima destroy  # tear down VM
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
--https` uses the same default CA directory and serves the embedded UI directly
at `https://localhost:<port>` while Vite proxies API requests over HTTPS.

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

| Variable | Purpose | Default |
|----------|---------|---------|
| `LLM_GATEWAY` | exe.dev LLM gateway URL | — |
| `ANTHROPIC_API_KEY` | Direct Anthropic API key (alternative to gateway) | — |
| `PHOENIX_PORT` | Server port | `8000` |
| `PHOENIX_DB_PATH` | SQLite database path | `~/.phoenix-ide/phoenix.db` |
| `PHOENIX_TLS` | HTTPS mode: `auto`/`on`/`true`/`1`, `manual`, or `off`/`none`/`false`/`0` | `off` |
| `PHOENIX_TLS_HOSTS` | Comma-separated extra DNS/IP SANs for `PHOENIX_TLS=auto` | `localhost,127.0.0.1,::1` |
| `PHOENIX_TLS_DIR` | Managed local CA and auto-issued leaf certificate directory | parent of `PHOENIX_DB_PATH` + `/tls` |
| `PHOENIX_TLS_CERT_PATH` | Manual TLS certificate PEM path; with key path, enables manual TLS even if `PHOENIX_TLS` is unset | — |
| `PHOENIX_TLS_KEY_PATH` | Manual TLS private key PEM path; required with cert path | — |
| `PHOENIX_PUBLIC_URL` | Display URL used by `./dev.py prod status`/deploy output; not read by the Rust server | derived from TLS mode |
| `RUST_LOG` | Log level (`info`, `debug`, …) | — |

TLS is opt-in. `PHOENIX_TLS=auto` creates a private Phoenix CA in
`PHOENIX_TLS_DIR` if one is not already present, then rotates the server leaf
certificate on startup. `PHOENIX_TLS=manual` serves the cert/key paths exactly as
configured; this is what `./dev.py tls install` writes for remote production
hosts. See [TLS.md](TLS.md) for the complete trust and deployment workflow.

## API Endpoints

- `GET /api/conversations` - List all conversations
- `POST /api/conversations/new` - Create new conversation
- `GET /api/conversations/:id` - Get conversation details
- `POST /api/conversations/:id/messages` - Send a message
- `GET /api/conversations/:id/stream` - SSE stream for real-time updates

## Documentation

- `specs/` — Per-tool and subsystem specs using the [spEARS methodology](SPEARS.md)
- [TLS.md](TLS.md) — HTTPS, HTTP/2, private CA, and deployment workflow
- [AGENTS.md](AGENTS.md) — Agent architecture and conventions
