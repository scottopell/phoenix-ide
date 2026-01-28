# PhoenixIDE

> Rising from the ashes - A ground-up rewrite

PhoenixIDE is a Rust implementation of an LLM agent development environment, built from the lessons learned from rustey-shelley and the original Go Shelley.

## Architecture

Built on the Elm Architecture pattern:
- **Pure state machine** at the core (`transition(state, event) -> (new_state, effects)`)
- **Effect executors** handle all I/O
- **Property-based testing** validates invariants

## Specifications

All design is driven by spEARS specifications:

- `specs/bedrock/` - Core conversation state machine
- `specs/bash/` - Bash tool (most critical tool)

## Core Principles

1. **Pure State Transitions** - All business logic in testable pure functions
2. **Immutable Working Directories** - Each conversation has a fixed working directory  
3. **Sub-Agents** - Parallel task execution with strict isolation (no nesting)
4. **Graceful Degradation** - Error recovery and server restart resilience

## MVP Features

- Conversation state machine with full lifecycle
- Bash tool with foreground/background execution
- Patch tool for file editing
- Think tool for reasoning
- Sub-agent spawning (no nesting)
- Shelley React UI compatibility (graceful feature degradation)
- SQLite persistence
- SSE real-time streaming

## Not in MVP

- Browser tools
- Context window management/continuation
- Multi-model switching within conversation

## Development

```bash
# Build
cargo build

# Test
cargo test

# Run
cargo run -- --port 8000
```

## Frontend

Uses the existing Shelley React UI. Features not implemented in MVP backend will be gracefully disabled.

---

*"From the ashes of chaos, clarity emerges"*
