# Agent Instructions for phoenix-ide

## What Is This?

LLM-powered coding agent. Rust backend (axum, SQLite) + React frontend (TypeScript, XState).

The core is a **state machine-driven conversation runtime**: messages flow through deterministic state transitions, tools execute as effects, and everything persists to SQLite for crash recovery.

## Architecture

```
crates/phoenix-ide/src/
  runtime/       # Conversation lifecycle, state machine executor
  state_machine/ # Pure state transitions (Elm architecture)
  tools/         # bash, patch, browser, keyword_search, think, tmux, etc.
  llm/           # Provider abstraction (Anthropic, OpenAI, Fireworks)
  api/           # HTTP handlers, SSE streaming
  db/            # SQLite persistence
  chain_runtime.rs / chain_qa/    # Phoenix Chains v1 (see specs/chains/)
  terminal/      # In-app terminal + tmux-attach bridging
crates/phoenix-tls/  # Terminal-line stream library (used by terminal/)
ui/src/
  components/    # React components
  machines/      # XState state machines
  hooks/         # Custom React hooks
  pages/         # Route-level components
specs/           # Behavioural specs (read before modifying anything spec'd!)
tasks/           # Task tracking
phoenix-client.py  # CLI client — interact with the app without a browser
```

`phoenix-client.py` is a standalone CLI for the Phoenix API (spec: `specs/simple_client/`). LLM agents should prefer it over browser automation for testing conversations.

**Specs** live under `specs/<name>/`: spEARS prose (`requirements.md`, `design.md`, `executive.md`) plus optional Allium behavioural specs (`.allium`). Both are normative. See [Specifications](#specifications) below.

---

## Task Tracking

**Create tasks with `taskmd new` — do not write task files directly.**

```bash
echo 'What the task does, in a few sentences.' \
  | taskmd new --slug fix-login --priority p1
```

`taskmd new` allocates the next ID, formats the filename, and atomically
writes the file with the body from stdin. Direct file writes (and
`taskmd next` + write-your-own) are discouraged because two callers
using `next` can race and receive the same ID.

If `taskmd` isn't on your PATH, `./dev.py taskmd …` runs the copy bundled
in dev.py's env — e.g. `… | ./dev.py taskmd new --slug fix-login --priority p1`.
Args are forwarded verbatim.

Required flag: `--slug`. Required input: a non-empty task body on stdin.
Optional: `--priority` (default `p2`), `--status` (default `ready`).

**Filename format** (produced by `taskmd new`, don't hand-craft):
`NNNNN-pX-status--slug.md` — e.g. `24691-p1-ready--fix-bug.md`.

- `pX`: `p0` (critical) … `p4` (nice-to-have)
- `status`: `ready`, `in-progress`, `blocked`, `brainstorming`, `done`, `wont-do`
- The filename is the **sole source of truth** for task metadata. Bodies
  are free-form markdown — no frontmatter. `./dev.py tasks validate`
  enforces filename pattern conformance and absence of duplicate IDs.

```bash
ls tasks/*-ready--*.md             # List ready tasks
taskmd list --status ready         # Same, structured
taskmd status <id> in-progress     # Transition a task by renaming the file
./dev.py tasks fix                 # Migrate legacy IDs / renumber duplicates
./dev.py tasks validate            # Check filenames + IDs (also runs in ./dev.py check)
```

---

## Issue Discovery Protocol

The trigger: you just noticed something. For example:

- A comment that contradicts the code.
- A legacy pattern left behind when the rest of the codebase moved on.
- A security check that's theater because another tool already bypasses it.
- An escape-hatch API that undermines a "correct by construction" design.
- A reducer branch that skips a helper all other branches go through.

### First question: is this my current work?

- **Yes** — on-path, or a tiny adjacent fix the user would expect → just do it.
- **No** — capture it, keep moving.

### Capturing: default to in-conversation TODO

`taskmd new` is for items that won't be addressed this session. During active work, an in-conversation TODO is lower friction and keeps context with the discussion.

### Example

Mid-QA on a sub-agent's commit you spot drift in a file that was out of their scope:

- **Right**: add to in-conversation TODO, finish QA, batch cleanup at session end.
- **Wrong**: interrupt QA with `taskmd new` for each observation — fragments review, dilutes the repo's task list.

### Never

- Silently delete regression files
- Say "this is unrelated" and move on without recording
- Leave mock data, hardcoded fixtures, or stub values committed

---

## Development

**Always use `./dev.py`** — it configures LLM gateway automatically.

```bash
./dev.py up          # Build and start Phoenix + Vite (auto-seeds DB if empty)
./dev.py down        # Stop all servers
./dev.py reap        # Kill dev servers orphaned by deleted worktrees (--dry-run to preview)
./dev.py restart     # Rebuild Rust, restart Phoenix (Vite keeps running)
./dev.py status      # Check what's running
./dev.py seed        # Populate dev DB with representative conversations (idempotent)
./dev.py check       # clippy + fmt + tests + task validation
```

**Workflow:** `./dev.py up` → make changes → `./dev.py restart` (Rust changes) or save (UI auto-reloads via Vite) → `./dev.py check` → commit

In dev mode, Vite serves `ui/` with hot reload. In production, `ui/dist/` is embedded into the Rust binary via RustEmbed.

Each git worktree gets unique ports and database automatically.

Deleting a worktree (e.g. via the harness) without running `./dev.py down` first orphans its Phoenix/Vite servers — the `.pid` files live inside the worktree and vanish with it, but the processes keep running and holding their ports. To survive this, each spawned server is also recorded in `~/.phoenix-ide/dev-registry/` (outside any worktree). `./dev.py up` auto-reaps such orphans before starting, and `./dev.py reap` cleans them on demand. A process is only killed when its working directory points into a now-deleted worktree, so live servers are never touched.

Phoenix must treat the user's Git worktrees as owned environments: fetching remote refs is safe, but moving a local branch ref that is checked out in any worktree is not. Before any operation that updates `refs/heads/*`, check all worktrees and skip the ref move if the branch is checked out.

**Logs:** Dev server logs to `phoenix.log` in the project root. Production logs to `~/.phoenix-ide/prod.log`.

⚠️ Do NOT use `cargo run` directly—server needs LLM gateway config from `./dev.py`.

If you're in a Claude Code remote sandbox (`IS_SANDBOX=yes`) and need to drive the UI in a real browser, `npx agent-browser` works once pointed at the pre-installed Playwright Chrome and given cert/sandbox args:

```bash
export AGENT_BROWSER_EXECUTABLE_PATH=/opt/pw-browsers/chromium-1194/chrome-linux/chrome
export AGENT_BROWSER_ARGS="--ignore-certificate-errors,--disable-dev-shm-usage,--no-sandbox"
npx agent-browser open http://localhost:8042
```

### Node + pnpm

The UI uses pnpm via Corepack, pinned in `ui/package.json#packageManager`.
`./dev.py` validates the corepack/pnpm versions on every run and prints
an actionable hint when something is wrong — follow the hint, no separate
bootstrap doc to keep in sync.

To bump the pnpm version: edit `packageManager` in `ui/package.json`, run
`pnpm install` to regenerate `pnpm-lock.yaml`, commit both.

---

## Commits and pushes

**Agents are authorized to commit in this repo without asking.** Commits are local and reversible; holding working-tree changes uncommitted across a long session costs more than it saves. (`./dev.py prod deploy` does warn loudly about dirty state but builds from HEAD regardless — easy to miss at the end of a long build log.) Commit completed units of work as you go.

Prefer logical splits over a single kitchen-sink commit when concerns are distinct. Use conventional-commit-ish prefixes (`fix:`, `feat:`, `refactor:`, `build:`, `tasks:`, `docs:`) matching the existing log style.

**Agents are authorized to commit freely and to push to the branch they're working on without asking.** Pushing a bug fix or completed unit of work is routine business — don't gate it behind a confirmation, including a `--force-with-lease` to land a rebase on a branch you own.

**Destructive remote operations remain prohibited** without explicit authorization: lease-less force-pushes, deleting remote branches or tags, or rewriting history on `main` or a branch you don't own. When a push would affect work that isn't yours, ask first.

---

## Testing

```bash
cargo test                       # All tests
cargo test state_machine         # Filter by module/name
cargo test -- --nocapture        # See println! output
```

Property tests live in `**/proptests.rs` files. Run with `cargo test proptests`.

---

## TypeScript codegen for SSE types (task 02677)

The SSE wire format is typed on the Rust side in [`src/api/wire.rs`](src/api/wire.rs) (`SseWireEvent`). `#[derive(ts_rs::TS)]` emits the matching TypeScript under `ui/src/generated/` during `cargo test` — those files are checked into git. The valibot schemas in `ui/src/sseSchemas.ts` are annotated `satisfies v.GenericSchema<unknown, WireInitData>` etc., so a Rust-side change surfaces as a tsc error until the schema is updated.

```bash
./dev.py codegen        # Regenerate ui/src/generated/ (fast path)
./dev.py check          # Full check including codegen-stale guard
```

`./dev.py check` runs `git diff --exit-code -- ui/src/generated/` after the Rust tests; a dirty diff means a developer edited a typed SSE struct without regenerating. The generated files should **never** be hand-edited — their headers say so.

Types that feed codegen need `#[derive(ts_rs::TS)]` + `#[ts(export, export_to = "../ui/src/generated/")]`. Types that are referenced but intentionally left opaque on the TS side (e.g. `MessageContent`, `ConvState`) are annotated `#[ts(type = "unknown")]` at their reference site.

Byte-for-byte wire parity with the pre-typed `json!()` path is guarded by the `parity_*` tests in `crates/phoenix-ide/src/api/sse.rs`.

---

## Adding a New Tool

See [`crates/phoenix-ide/src/tools/think.rs`](crates/phoenix-ide/src/tools/think.rs) as the simplest example.

1. Create `crates/phoenix-ide/src/tools/your_tool.rs` implementing the `Tool` trait:
   - `name()` — tool identifier
   - `description()` — shown to LLM
   - `input_schema()` — JSON schema for parameters
   - `run()` — async execution, returns `ToolOutput`

2. Register in `crates/phoenix-ide/src/tools.rs` → `ToolRegistry::new_with_options()`

3. Add spec in `specs/your-tool/executive.md` (see existing specs for format)

**Before modifying any existing tool**, read its spec in `specs/<tool>/executive.md`.

---

## Specifications

This project uses two complementary specification formats. **Both are normative** — code that contradicts either is wrong.

- **spEARS** specs (`requirements.md`, `design.md`, `executive.md`) capture the *what* and *why*, plus the high-level *how*. They establish user need, named requirements (REQ-* IDs), design rationale, and track implementation status.
- **Allium** specs (`.allium` files) capture *how specifically* — states, transitions, preconditions, postconditions, invariants — precisely enough to generate tests and catch ambiguities.

The two formats complement each other. spEARS without Allium is vague about exact behaviour; Allium without spEARS is unmoored from user need. Together: user story → REQ-IDs → precise behavioural spec → testable implementation → status tracking, traceable end-to-end.

### When to write an Allium spec alongside spEARS

Allium is a heavier tool; not every spEARS spec needs an Allium counterpart. Write one when the system has:

- **State machines** with multiple states and complex transitions (bedrock, projects)
- **Lifecycle flows** with preconditions that must hold (task approval, complete, abandon)
- **Multi-step operations** where ordering matters and partial failure is possible
- **Cross-boundary contracts** where two specs interact (projects importing bedrock)

Do NOT add Allium for: CRUD endpoints, pure data transformations, UI components, tool implementations with no lifecycle. spEARS alone is sufficient there.

### Discovering specs

Both formats live under `specs/<name>/`:

- spEARS: `requirements.md`, `design.md`, `executive.md`
- Allium: `<name>.allium`

Enumerate Allium specs with `ls specs/*/*.allium`. Cross-spec dependencies are declared in each file's header via `use "./other.allium" as other`. Validate with `allium check specs/<name>/<name>.allium` (install via `cargo install allium-cli`).

### Working with Allium specs

```bash
# Distill a new spec from existing code
/allium:distill

# Generate tests from a spec
/allium:propagate
```

**Resolving open questions is mandatory.** An open question in an Allium spec is not documentation — it's an unresolved ambiguity that may hide a bug. When distilling, present each open question to the user via `AskUserQuestion` with concrete options (not open-ended). The user decides; you implement the fix. Do not leave open questions as prose notes or "future work." Every ambiguity either becomes a code fix or an explicit design decision before the spec is merged.

**Both formats are authoritative.** If the code disagrees with either spEARS or Allium, one of them is wrong. spEARS requirements (REQ-* IDs) define what must be built; Allium's transition graph, preconditions, and invariants define exact behaviour. `@guidance` blocks in Allium describe implementation sequences — if the code's sequence differs, investigate before assuming the code is right.

**Specs are timeless.** A spec describes the ideal current state of the system as if it had always been that way — it is the guidebook a future reader uses to understand design *intent*, not a record of how the design got here. Write every spec as a standing description, never as a changelog or a snapshot of in-flight work. Concretely, the following do **not** belong in a spec file:

- **Task / PR / issue references** as the reason for a behaviour — `task 02679`, `PR #155`, `see #186`. Cite the *invariant* or *bug class* in timeless terms instead ("an emit-vs-persist race that drops a finalized message"), and cross-reference other **specs** by path, not tasks by ID.
- **Time- or state-relative framing** of the design — `currently`, `for now`, `recently`, `previously`, `used to`, `will soon`, `Phase 1 (current)`, `landed in tasks/62001`, `MVP`. State what *is*. (Sequential phases *within a single operation* — "phase 1: snapshot, phase 2: apply pending" — are fine; they describe algorithm structure, not a rollout schedule.)
- **Status / progress tracking** — `✅ Complete`, `Progress: 10 of 10`, "implemented in", per-rule completion columns. Implementation status lives in tasks and git, not the spec. A requirement-to-surface or rule-to-code-anchor map is fine; a *status* table is not.
- **Decision logs and resolved "Open Questions"** — `Q3. RESOLVED 2026-05-10: …`, dated entries, "we decided", stream-of-consciousness ("actually no — see below"). Once a question is resolved, state the decision **as a fact** with its rationale (a "Design Decisions" section), and delete the question. An *unresolved* open question is never left as prose — resolve it per the rule above.
- **Line-number citations that rot** — prefer symbol names (`SseBroadcaster::send_seq`) over `runtime.rs:529`. See `specs/AUTHORING.md` §2.

When you touch a spec, leave it more timeless than you found it, even for drift you didn't introduce.

**Before pushing a spec change**, run the pre-flight checklist in [`specs/AUTHORING.md`](specs/AUTHORING.md). The checklist captures the failure modes that drove 8 rounds of review iteration on PR #155 (wire-shape mismatches, Allium grammar bugs, undeclared helpers, cross-file drift, stale citations, cross-spec whitelist gaps) so future spec authors don't repay them.

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
