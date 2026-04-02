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
phoenix-client.py  # CLI client — interact with the app without a browser
```

`phoenix-client.py` is a standalone CLI for the Phoenix API (spec: `specs/simple_client/`). LLM agents should prefer it over browser automation for testing conversations.

---

## Task Tracking

**Format:** `NNNN-pX-status--slug.md` (e.g., `0042-p1-ready--fix-bug.md`)

- `NNNN`: 4-digit task number | `pX`: Priority (p0 highest) | `status`: ready, in-progress, done, blocked, etc.

```bash
ls tasks/*-ready--*.md             # List ready tasks
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

**Workflow:** `./dev.py up` → make changes → `./dev.py restart` (Rust changes) or save (UI auto-reloads via Vite) → `./dev.py check` → commit

**After any Rust change, always run `./dev.py restart`** and give the user the UI URL from its output so they can immediately verify. UI-only changes (`.tsx`, `.css`) hot reload via Vite — no restart needed, but still tell the user the URL. The user should never have to ask for a restart or wonder if their running server has the latest code.

In dev mode, Vite serves `ui/` with hot reload. In production, `ui/dist/` is embedded into the Rust binary via RustEmbed.

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

## Code Correctness Principles

These are constraints on the technical artifact, not process guidelines. They override existing code patterns and unreviewed plan decisions. When a plan says to do something that violates these, deviate from the plan and note why.

### Correct-by-construction is the governing principle

Design so invalid states cannot be structurally represented. If a type permits a value that is semantically wrong, the type is wrong — fix the type, not the discipline. Runtime checks, comments, and conventions that rely on human vigilance are not substitutes.

```rust
// ❌ Bad: String is valid whether it holds JSON, base64, or a summary — wrong states representable
pub output: String

// ✅ Good: enum makes the distinction structural and compiler-enforced
pub enum ToolOutputContent {
    Summary(String),
    Image { media_type: String, data: String },
}
```

### Omission is data loss — unless the component is a typed sink

If a field exists in struct A and struct B is the next layer that accepts that kind of data, threading it through is required. A component *may* be an intentional consumer/terminator of a value, but this must be enforced by its type — not by implicit omission or a comment. There must be no structural ambiguity between "forgot to thread" and "deliberately consumed."

```rust
// ❌ Bad: images: _ is structurally indistinguishable from "forgot to thread"
ContentBlock::ToolResult { images: _, ... } => { ... }

// ✅ Good: provider-specific types make the capability gap unrepresentable
// AnthropicToolResult carries images; OpenAIToolResult structurally cannot
```

### No parallel representations of the same semantic value

If data appears in two representations simultaneously, one is redundant. Redundant representations diverge and create ambiguity about which is authoritative. Each field carries data for exactly one consumer, with a non-overlapping contract.

```rust
// ❌ Bad: same image bytes in both display_data["data"] (JSON blob) and images[0].data (typed)
// — two representations, same value, divergence risk

// ✅ Good: display_data holds UI-only metadata (thumbnail URL, dimensions)
//          images holds typed LLM-bound data
//          Non-overlapping consumers, non-overlapping contracts
```

### Schema evolution belongs in migrations, not serde annotations

`#[serde(default)]` on a JSON-in-TEXT column field is a patch around a missing migration, not a solution. It is acceptable as a backward-compat shim *during rollout*, but the migration must exist or be tracked as a task. The decision to store structured data as a JSON TEXT blob must be explicitly owned, not treated as an inert constraint to work around.

```rust
// ❌ Bad: serde(default) as the complete schema change
#[serde(default)]
pub images: Vec<ToolContentImage>,  // old rows silently get empty vec, no migration, no auditability

// ✅ Good: migration adds typed column; serde(default) covers the rollout window only
// See db/migrations/ — every structural change to persisted data has a migration
```

### Capability gaps are logged, not silenced

When a component drops data because the backend does not support a feature, this must appear in logs at `debug` level or above. Silent omission is indistinguishable from a bug.

```rust
// ❌ Bad: images discarded, no trace in logs
ContentBlock::ToolResult { images: _, ... } => { ... }

// ✅ Good: visible in logs
if !images.is_empty() {
    tracing::debug!(n = images.len(), provider = "openai",
                    "dropping images from tool result — unsupported by this provider");
}
```

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
