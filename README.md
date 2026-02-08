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

## Environment Variables

| Variable | Purpose | Default |
|----------|---------|---------|
| `LLM_GATEWAY` | exe.dev LLM gateway URL | — |
| `ANTHROPIC_API_KEY` | Direct Anthropic API key (alternative to gateway) | — |
| `PHOENIX_PORT` | Server port | `8000` |
| `PHOENIX_DB_PATH` | SQLite database path | `~/.phoenix-ide/phoenix.db` |
| `RUST_LOG` | Log level (`info`, `debug`, …) | — |

## API Endpoints

- `GET /api/conversations` - List all conversations
- `POST /api/conversations` - Create new conversation
- `GET /api/conversations/:id` - Get conversation details
- `POST /api/conversations/:id/messages` - Send a message
- `GET /api/conversations/:id/events` - SSE stream for real-time updates

## Documentation

- `specs/` — Per-tool and subsystem specs using the [spEARS methodology](SPEARS.md)
- [AGENTS.md](AGENTS.md) — Agent architecture and conventions
