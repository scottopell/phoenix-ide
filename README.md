# Phoenix IDE

A Rust backend for LLM-powered development environments, designed for the exe.dev platform.

## Quick Start

```bash
# Build
cargo build --release

# Run with exe.dev LLM gateway
LLM_GATEWAY="http://169.254.169.254/gateway/llm" ./target/release/phoenix_ide

# Or with direct Anthropic API key
ANTHROPIC_API_KEY="your-key" ./target/release/phoenix_ide
```

Server runs on port 8000 by default.

## Using the Python Client

```bash
# Install dependencies (if needed)
pip3 install httpx click --break-system-packages

# Create a new conversation and send a message
python3 phoenix-client.py -d /tmp "Create hello.txt with 'Hello World'"

# Continue an existing conversation
python3 phoenix-client.py -c <conversation-slug> "Now modify it to say 'Hello Phoenix'"
```

## Architecture

- **State Machine**: Manages conversation lifecycle (Idle → Processing → ToolExecuting → etc.)
- **Tool System**: Modular tools (bash, patch, think, keyword_search, read_image)
- **LLM Integration**: Supports Anthropic Claude models via direct API or exe.dev gateway
- **SQLite Database**: Persists conversations and messages

### Patch Tool

The patch tool uses an Effect/Command pattern for property-based testing:

```
src/tools/patch/
├── types.rs       # Core types (Operation, PatchRequest, etc.)
├── matching.rs    # Text matching logic (exact, dedent, trimmed)
├── planner.rs     # Pure patch planning (no IO)
├── executor.rs    # Filesystem IO operations
├── interpreter.rs # In-memory effect interpreter for testing
└── proptests.rs   # Property-based tests (6 invariants, 500 cases each)
```

## Tests

```bash
# Run all tests
cargo test --release

# Run patch tool demo
./demo_patch_fix.sh
```

## Environment Variables

- `LLM_GATEWAY`: exe.dev LLM gateway URL (e.g., `http://169.254.169.254/gateway/llm`)
- `ANTHROPIC_API_KEY`: Direct Anthropic API key (alternative to gateway)
- `PHOENIX_PORT`: Server port (default: 8000)
- `PHOENIX_DB_PATH`: Database path (default: `~/.phoenix-ide/phoenix.db`)
- `RUST_LOG`: Log level (e.g., `info`, `debug`)

## API Endpoints

- `GET /api/conversations` - List all conversations
- `POST /api/conversations` - Create new conversation
- `GET /api/conversations/:id` - Get conversation details
- `POST /api/conversations/:id/messages` - Send a message
- `GET /api/conversations/:id/events` - SSE stream for real-time updates
