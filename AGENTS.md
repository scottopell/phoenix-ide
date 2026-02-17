# Agent Instructions for phoenix-ide

## What Is This?

LLM-powered coding agent. Rust backend (axum, SQLite) + React frontend (TypeScript, XState).

The core is a **state machine-driven conversation runtime**: messages flow through deterministic state transitions, tools execute as effects, and everything persists to SQLite for crash recovery.

## Architecture

```
src/
  runtime/       # Conversation lifecycle, state machine executor
  state_machine/ # Pure state transitions (Elm architecture)
  tools/         # bash, patch, browser, keyword_search, think, etc.
  llm/           # Provider abstraction (Anthropic, OpenAI, Fireworks)
  api/           # HTTP handlers, SSE streaming
  db/            # SQLite persistence
ui/src/
  components/    # React components
  machines/      # XState state machines
  hooks/         # Custom React hooks
specs/           # Tool specifications (read before modifying tools!)
tasks/           # Task tracking
```

---

## Task Tracking

**Format:** `NNN-pX-status-slug.md` (e.g., `042-p1-ready-fix-bug.md`)

- `NNN`: Task number | `pX`: Priority (p0 highest) | `status`: ready, in-progress, done, blocked, etc.

```bash
ls tasks/*-ready-*.md              # List ready tasks
./dev.py tasks fix                 # Sync filenames to frontmatter
./dev.py tasks validate            # Check consistency (runs in ./dev.py check)
```

Filename MUST match frontmatter. `./dev.py check` enforces this.

---

## Issue Discovery Protocol

> **Finding a bug is the beginning of a task, not an observation to note and move on.**

When you encounter ANY issue—related to your current work or not:
1. **Create a task** in `tasks/` with reproduction steps and context
2. Then continue with your original work

Do NOT delete regression files, work around problems silently, or say "this is unrelated."

---

## Development

**Always use `./dev.py`** — it configures LLM gateway automatically.

```bash
./dev.py up          # Build and start Phoenix + Vite
./dev.py down        # Stop all servers
./dev.py restart     # Rebuild Rust, restart Phoenix (Vite keeps running)
./dev.py status      # Check what's running
./dev.py check       # clippy + fmt + tests + task validation
```

**Workflow:** `./dev.py up` → make changes → `./dev.py restart` (Rust) or auto-reload (UI) → `./dev.py check` → commit

Each git worktree gets unique ports and database automatically.

⚠️ Do NOT use `cargo run` directly—server needs LLM gateway config from `./dev.py`.

---

## Testing

```bash
cargo test                       # All tests
cargo test state_machine         # Filter by module/name
cargo test -- --nocapture        # See println! output
```

Property tests live in `**/proptests.rs` files. Run with `cargo test proptests`.

---

## Adding a New Tool

See [`src/tools/think.rs`](src/tools/think.rs) as the simplest example.

1. Create `src/tools/your_tool.rs` implementing the `Tool` trait:
   - `name()` — tool identifier
   - `description()` — shown to LLM
   - `input_schema()` — JSON schema for parameters
   - `run()` — async execution, returns `ToolOutput`

2. Register in `src/tools.rs` → `ToolRegistry::new_with_options()`

3. Add spec in `specs/your-tool/executive.md` (see existing specs for format)

**Before modifying any existing tool**, read its spec in `specs/<tool>/executive.md`.

---

## Production

```bash
./dev.py prod deploy [version]   # Build + install systemd service
./dev.py prod status             # Show status
./dev.py prod stop               # Stop service
```

Builds static ~9MB binary with embedded UI. Runs on port 8031, database at `~/.phoenix-ide/prod.db`.

---

## Code Conventions

### Module Organization

Use `foo.rs` + `foo/` subdirectory, NOT `foo/mod.rs`. Enforced by clippy.

```
✅ src/tools.rs + src/tools/bash.rs
❌ src/tools/mod.rs + src/tools/bash.rs
```

---

## UI Design Philosophy

### Information Density, Not Minimalism

- Show status inline (e.g., `DIR ✓ ~/project` — validity and value in one glance)
- Use symbols and color to convey state without words
- Progressive disclosure: essentials visible, details on demand

### Input-First Design

- Primary action (message input) dominates the interface
- Settings collapsed by default
- Remember user preferences (last directory, model)

### Feedback Patterns

| State | Pattern |
|-------|--------|
| Valid/Success | Green `✓` |
| Will be created | Yellow `+` |
| Invalid/Error | Red `✗` |
| Loading | Muted `...` |

Status indicators go **inline** with the value they describe.

### Animation

- Quick (150-250ms) and purposeful
- No bounces or playful effects—professional tool
- Never block user input

### The Test

Before adding UI: (1) What info does this communicate? (2) Is it already shown elsewhere? (3) Does the user need it?

If #2=yes or #3=no, don't add it.
