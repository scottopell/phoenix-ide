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

**Allium specs** (`.allium` files in `specs/`) are formal behavioral specifications that complement spEARS prose. See [Behavioral Specifications](#behavioral-specifications-allium) below.

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

**Logs:** Dev server logs to `phoenix.log` in the project root. Production logs to `~/.phoenix-ide/prod.log`.

⚠️ Do NOT use `cargo run` directly—server needs LLM gateway config from `./dev.py`.

---

## Commits and pushes

**Agents are authorized to commit in this repo without asking.** Commits are local and reversible; holding working-tree changes uncommitted across a long session costs more than it saves. (`./dev.py prod deploy` does warn loudly about dirty state but builds from HEAD regardless — easy to miss at the end of a long build log.) Commit completed units of work as you go.

Prefer logical splits over a single kitchen-sink commit when concerns are distinct. Use conventional-commit-ish prefixes (`fix:`, `feat:`, `refactor:`, `build:`, `tasks:`, `docs:`) matching the existing log style.

**Push still requires explicit user authorization.** Pushes affect others and trigger deploys; keep them opt-in.

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

## Behavioral Specifications (Allium)

spEARS specs (`requirements.md`, `design.md`, `executive.md`) capture *why* something should be built and track implementation status. Allium specs (`.allium` files) capture *what exactly* the system does — states, transitions, preconditions, postconditions, invariants — precisely enough to generate tests and catch ambiguities.

**spEARS without Allium:** rigorous about whether to build, vague about what exactly to build.
**Allium without spEARS:** precise about behaviour, unmoored from user need and project reality.
**Together:** user story → requirement ID → precise behavioral spec → testable implementation → status tracking, all traceable end-to-end.

### When to use Allium

- **State machines** with multiple states and complex transitions (bedrock, projects)
- **Lifecycle flows** with preconditions that must hold (task approval, complete, abandon)
- **Multi-step operations** where ordering matters and partial failure is possible
- **Cross-boundary contracts** where two specs interact (projects importing bedrock)

Do NOT use for: CRUD endpoints, pure data transformations, UI components, tool implementations with no lifecycle.

### Current Allium specs

| Spec | Imports | Scope |
|------|---------|-------|
| `specs/bedrock/bedrock.allium` | — | Conversation state machine (14 states, 48 rules) |
| `specs/projects/projects.allium` | bedrock | Project lifecycle, git operations (12 rules) |

### Working with Allium specs

```bash
# Distill a new spec from existing code
/allium:distill

# Generate tests from a spec
/allium:propagate
```

**Resolving open questions is mandatory.** An open question in an Allium spec is not documentation — it's an unresolved ambiguity that may hide a bug. When distilling, present each open question to the user via `AskUserQuestion` with concrete options (not open-ended). The user decides; you implement the fix. Do not leave open questions as prose notes or "future work." Every ambiguity either becomes a code fix or an explicit design decision before the spec is merged.

**The spec is authoritative for behavior.** If the code disagrees with the Allium spec, one of them is wrong. The transition graph, preconditions, and invariants in the `.allium` file define correct behavior. `@guidance` blocks describe implementation sequences — if the code's sequence differs, investigate before assuming the code is right.

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

### Comments are local facts, not distributed specifications

A comment is safe when it describes a local fact about the line it's on. A comment is dangerous when it describes a design decision, an invariant, or an operation sequence that could silently become wrong.

**The test:** "If this comment becomes false, will anything fail?" If the answer is no, the comment is a liability — it will eventually lie, and the lie will make the next reader skip the code path that contains the bug.

**Keep:**
```rust
// --force required: worktree may have uncommitted files
run_git(cwd, &["worktree", "remove", &path, "--force"])?;

// serde(default) rollout shim — migration tracked in task 0087
#[serde(default)]
pub worktree_path: String,
```

**Move to spec, then delete:**
```rust
// ❌ Design rationale belongs in spEARS design.md or Allium @guidance
// "Commit after worktree creation so a worktree failure
//  doesn't leave orphaned commits on main"

// ❌ Invariant belongs in Allium invariant block
// "pending.count + completed.count = total spawned"

// ❌ Operation sequence belongs in Allium @guidance
// "Sequence: checkout base_branch, merge --squash, update task file, commit"
```

**Delete outright:**
```rust
// ❌ Restates what the code does
// Stage the task file
run_git(cwd, &["add", &relative_path])?;

// ❌ Section divider with no information
// ============ Tool Execution ============
```

When an Allium spec exists for a module, the spec is the authoritative source for design rationale, invariants, and operation sequences. Comments in the code that duplicate spec content will diverge and mislead. If the spec doesn't exist yet, a comment is acceptable as a stopgap, but it must be migrated when the spec is created.

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
