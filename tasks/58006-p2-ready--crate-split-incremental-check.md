# Stage 2 & 3: crate-split the phoenix-ide monolith for incremental `./dev.py check`

## Why this task exists

`./dev.py check`'s wall-clock is ~95% Rust **compilation**, not test execution
(measured cold: clippy ~142s + test-compile ~319s vs. test-run ~26s). The
root cause is that `phoenix-ide` is a **single binary crate**: any `.rs` edit
recompiles all ~94k LOC and runs clippy/tests workspace-wide. Splitting it
into library crates lets Cargo recompile only the changed crate + its
dependents, and lets the check run clippy/nextest on only the affected crates.

Prerequisite work is **already done** (see git log on
`claude/incremental-test-running-MzhDB`):

- Path-gating in `./dev.py check` (skips whole lanes when their inputs are
  unchanged; markdown-only check went 488s -> 1.9s). This is the win for
  *non-Rust* changes and is independent of this task.
- `phoenix-core` extracted as an acyclic base crate holding the shared domain
  model (`domain::{llm_types, db_schema, sm_state, sm_event, bash_types,
  patch_types, kill_signal, retry_policy, mode_context, skill_invocation}`
  plus leaves `llm_language, task_source, work_scope, platform`). This broke
  the `db <-> state_machine` type cycle that previously made any crate split
  impossible.

## READ THIS FIRST: Stage 2 and Stage 3 are a package deal

Neither stage delivers the headline goal ("near-instant `./dev.py check` on a
Rust edit") on its own:

- **Stage 2 alone** (split into crates) speeds up *incremental local builds*,
  but `./dev.py check` still invokes `cargo clippy`/`cargo test` at the
  **workspace** level, so check wall-clock barely moves. Net check-time win ~ 0.
- **Stage 3 alone** (per-crate gating in dev.py) has **nothing to narrow** while
  everything is one crate. Net win = 0.
- **Together**: a one-line edit in (say) `tools` checks `tools` + its dependents
  only — the Bazel-like outcome, on stable Cargo.

Corollary: do not land Stage 2 and declare victory. If Stage 2 is abandoned
part-way, every partial crate split still compiles (each extraction is a
validated checkpoint), but the build-time payoff stays ~0 until Stage 3 lands
on top of a meaningfully-split workspace. **Budget for both or neither.**

Exception: Stage 2 has standalone *architectural* value (compiler-enforced
layer boundaries, smaller blast radius, clearer dependency graph). If that is
the goal, Stage 2 alone is defensible — but then say so explicitly and do not
justify it on check-time grounds.

## De-risk first: the measured spike (recommended entry point)

Before committing to all six crate extractions, extract the single fattest
module and measure the real delta:

1. Pick `tools` (~25.7k LOC, the largest module, and a relatively leaf-ish
   consumer). Extract it to `phoenix-tools` following the recipe below.
2. Add a minimal Stage 3 hook: when only `crates/phoenix-tools/**` changed, run
   `cargo clippy -p phoenix-tools` + `cargo nextest run -E 'rdeps(phoenix-tools)'`
   instead of the workspace lane.
3. Measure, on a warm target, a representative one-line edit in `tools`:
   `time cargo clippy` (workspace) vs `time cargo clippy -p phoenix-tools`,
   and the same for test-compile. Record numbers in this task.
4. Decision gate: if the incremental delta is close to the projection
   (editing `tools` no longer recompiles `api`/`runtime`/`state_machine`),
   continue peeling the rest. If not, stop — one crate's effort spent, not six,
   and the banked gating + cleaner architecture remain.

## Stage 2 — split the logic modules into crates

### Target DAG

```
phoenix-core            (done) domain types + leaves
  |-- phoenix-llm        (core)            providers, registry, error taxonomy
  |-- phoenix-terminal   (core)            tmux/line-stream core (NOT ws.rs)
  |-- phoenix-db         (core, llm)       persistence logic
  |-- phoenix-tools      (core, llm, terminal)
  |-- phoenix-state-machine (core, llm, tools, db-types-via-core)
  |-- phoenix-runtime    (db, state_machine, tools, llm, terminal)
  `-- phoenix-api        (everything)  -->  phoenix-ide bin (thin main.rs)
```

Module sizes (LOC, for sequencing by payoff): tools 25.7k, api 13.3k,
runtime 12.1k, llm 11.6k, state_machine 11.1k, db 6.3k, terminal 4.2k.

### Step 2.0 — break the remaining wrong-way edges (makes the module graph a DAG)

These are the shallow inversions catalogued during scoping. Do them first so
each crate extraction is a clean lift:

- **llm -> api** (`crate::api::ModelInfo` in `llm/registry.rs` ~770-799):
  invert. Build the model-info data in `llm` (a `llm`-owned struct or a
  `phoenix-core` type), map it to the `api::ModelInfo` wire type *in* `api`.
- **db -> runtime** (`crate::runtime::TaskApprovalHandoffData`, used in
  `db.rs` fn signatures): move `TaskApprovalHandoffData` to
  `phoenix-core` (pure POD; it already only references `task_source::Priority`,
  which is in core). Re-export from `runtime` at the old path.
- **terminal -> {api, runtime}** (`terminal/ws.rs` uses `AppState`,
  `RuntimeManager`, `SseEvent`): `ws.rs` is api/runtime-layer glue misfiled
  under `terminal`. Relocate it up into `api` (or `runtime`); the terminal
  *core* (tmux, line streams) stays low and depends only on `core`.
- **chain_runtime -> runtime** (3 refs): all are `///` doc-links, not compile
  edges. Downgrade to plain text or cross-crate intra-doc links.

Validate the module graph is acyclic (still one crate at this point) with a
full `./dev.py check` before extracting anything.

### Step 2.1..2.7 — extract crates bottom-up

Extract in dependency order so each new crate's deps are already crates (or
core). Suggested order: `llm` -> `terminal` -> `db` -> `tools` ->
`state_machine` -> `runtime` -> (`api` either becomes `phoenix-api` or stays as
the bin). Each extraction is one commit and follows the **recipe** below.

### Per-extraction recipe (proven on increments 2a/2b)

1. `git mv` the module's files into `crates/phoenix-<name>/src/`.
2. Create `Cargo.toml` (edition 2021, `rust-version = "1.94"`, `[lints]
   workspace = true`); add only the external deps that module actually uses.
3. Add the crate to the workspace `members` and as a path dependency of the
   crates that need it.
4. **Move-down, re-export-up**: in the parent that used to own the module,
   replace `mod <name>;` with `pub use phoenix_<name>::*` (or a module alias
   `pub use phoenix_<name> as <name>;`) so existing `crate::<name>::...` call
   sites resolve unchanged. Defer call-site rewrites to a later optional pass.
5. Rewrite intra-moved-file cross-refs to the new crate paths.
6. Validate (all must pass):
   - `cargo check -p phoenix-<name>` then `-p phoenix_ide`
   - `cargo clippy --fix -p phoenix-<name> --lib --allow-dirty` to auto-apply
     the `must_use_candidate` / `missing_errors_doc` lints that fire when
     binary-private code becomes public library API, then `cargo clippy`
     clean on both crates.
   - `cargo fmt` (clippy --fix can leave trailing whitespace).
   - `PHOENIX_SKIP_BROWSER_TESTS=1 PHOENIX_SKIP_NETWORK_TESTS=1 cargo test
     --workspace` green (browser/Chrome + network failures are environmental,
     not regressions).
   - `git status --porcelain -- ui/src/generated/` empty (no ts-rs codegen
     drift).
7. Commit; push.

### Known gotchas (hit during increment 2 — expect them again)

- **Cross-crate `#[cfg(test)]`**: a `cfg(test)` item is invisible across a
  crate boundary. Any test-only helper in crate X used by crate Y's tests must
  be gated `#[cfg(any(test, feature = "test-support"))]`, with X exposing a
  `test-support` feature and Y enabling it via dev-dependencies (off in
  production). `phoenix-core` already does this for `ContentBlock::tool_use`;
  the pattern will recur per crate.
- **ts-rs `export_to`** is manifest-dir-relative, and all `crates/*` sit at the
  same depth, so `"../../../ui/src/generated/"` resolves identically after a
  move — no path edits needed, but the new crate needs the `ts-rs` dep and the
  generated TS must come out byte-identical (the codegen-stale check enforces
  this).
- **New-crate clippy surface**: promoting modules to library API exposes them
  to pedantic public-API lints the binary was hiding (`must_use_candidate`,
  `missing_errors_doc`). Mostly auto-fixable; `missing_errors_doc` needs a
  one-line `# Errors` doc per `Result`-returning pub fn.

## Stage 3 — per-crate lane gating in `./dev.py check`

Builds on the existing path-gating. Compose, don't replace.

### Step 3.1 — map changed paths to changed crates

Extend the gating helper to translate `crates/phoenix-<name>/**` -> crate name.
Reuse the existing `_changed_paths_vs_base()`; add a `crate_for_path()` map.

### Step 3.2 — narrow the rust lane

Instead of workspace-wide `cargo clippy` / `cargo nextest run`:

- `cargo clippy -p <changed-crate> ...` for each changed crate.
- `cargo nextest run -E 'rdeps(<changed-crate>)'` to run the changed crate's
  tests **and its reverse-dependencies'** tests (a change can break dependents).
  `rdeps` is the correct selector here, not `deps`.
- If `phoenix-core` changed, fall back to the full workspace lane (everything
  depends on it) — or at minimum re-run the ts-rs codegen + codegen-stale guard.
- Keep the existing fail-safe: undeterminable base, or a `dev.py` change, runs
  everything. `--all` / `PHOENIX_CHECK_ALL=1` forces the full workspace lane.
  Prod deploy pre-checks always run full.

### Step 3.3 — codegen-stale interaction

The ts-rs export tests live wherever the typed structs live (now
`phoenix-core`, later possibly per-crate). The codegen-stale guard must run
whenever any crate that emits `export_bindings_*` changed. Simplest correct
rule: run codegen + the guard if `phoenix-core` (or any ts-rs-deriving crate)
is in the changed set.

## Acceptance / definition of done

- A one-line edit confined to a leaf crate makes `./dev.py check` compile and
  lint/test only that crate + its dependents; measured wall-clock recorded in
  this task and compared against the pre-split baseline.
- Editing `phoenix-core` still triggers a full check (correctly conservative).
- Full `./dev.py check` (no gating, `--all`) stays green end to end.
- No ts-rs codegen drift; no weakened lints (auto-fixed, not `allow`-ed,
  except where a types-crate-wide `allow` is explicitly justified).

## Explicitly out of scope / rejected

- **cargo-difftests** (coverage-based per-test selection): optimizes the ~26s
  test-execution while *adding* overhead to the ~461s compile, requires nightly
  + a patched upstream tool + fail-open per-test instrumentation. Wrong
  bottleneck for this project. Do not pursue unless test execution becomes the
  dominant cost.
- **Deeper db refactor** (separating `db/schema.rs`'s persistence fns from the
  pure types now in core) is a known follow-up but is an architecture play, not
  a build-time one — track separately.

## Spike results

Ran the de-risking spike. Outcome: **crate-splitting delivers the projected
incremental-compile win, and the per-crate gating mechanism works** — but the
fattest target (`tools`) needs one bounded dependency inversion before it can
be extracted. Recommendation: **continue**, doing the LLM-trait inversion next.

### What landed (committed on the branch)

1. **Cycle-break: bash/tmux wire types → phoenix-core.** `tools` imported 13
   `#[ts(export)]` bash/tmux response types from `api::wire` (a real
   production `tools → api` cycle, not the doc-link it was first scoped as).
   Moved them to `phoenix_core::domain::tool_wire`; generated TS byte-identical.
2. **Extracted `phoenix-terminal`** (~4.2k LOC). The axum/WebSocket glue
   (`terminal/ws.rs`, 912 LOC, depends on `api::AppState`/`runtime`) was *not*
   terminal-core; relocated up to `api::terminal_ws` to avoid re-creating a
   cycle. `phoenix-terminal` is acyclic (no `api`/`runtime` refs).

### Measurements (warm target, representative one-line edit, this 4-vCPU box)

Incremental cost of an edit, per the affected crate:

| Edit location | `clippy -p <crate>` (split) | `clippy --workspace` (today) |
|---|---|---|
| inside `phoenix-terminal` (a real crate) | **3.4s** | 39.7s |
| inside `phoenix-core` (base) | 3.0s (then rdeps) | — |
| inside `phoenix-ide` (still the monolith) | 15.0s | 11.9s |

Test-compile, edit inside `phoenix-terminal`: **1.4s** (`-p`) vs **14.4s**
(`--workspace --no-run`).

Reading the numbers honestly:
- A change confined to an extracted crate checks in **~3s vs ~40s** — an order
  of magnitude, and exactly the Bazel-like outcome the task set out to test.
- The `phoenix-ide` row (15s `-p` ≈ 12s workspace) is the **control**: editing
  the still-monolithic crate gains nothing from `-p`, because it *is* the bulk.
  This is expected and is the whole argument for continuing to peel crates out
  of it — every module moved into its own crate converts a "12s+ workspace
  recompile" edit into a "~3s single-crate" edit.
- Net: the win is **proportional to how much code lives in right-sized crates**.
  Two crates (`core`, `terminal`) prove the mechanism; the payoff scales as the
  big modules (`tools`, `llm`, `runtime`, `state_machine`, `api`) follow.

### Blocker found (the reason `tools` itself didn't extract this round)

`tools` is not independently extractable yet:
- `ToolContext` (the shared tool spine) holds `llm_registry: Arc<ModelRegistry>`
  as a **concrete field**, and `keyword_search.rs` calls the live LLM via
  `LlmService` (production). `ModelRegistry` (1.5k LOC) + the `LlmService` trait
  live in phoenix-ide's `llm` module (11k LOC), and `registry.rs` has its own
  upward edge `→ api::ModelInfo`. So `tools` can't point cleanly down to core.
  (All 18 `ModelRegistry::new_empty()` calls *in tools* are `#[cfg(test)]`; only
  the `ToolContext` field + `keyword_search` are production.)
- Minor, easy: `tools/skill.rs` calls `system_prompt::discover_skills` +
  `skills::invoke_skill` (one file, two fns); `SkillInvocation` is already in
  core, `SkillMetadata` + the two fns are a bounded move.

### Recommended next step: invert the LLM dependency (then `tools` is unblocked)

Define `LlmService` + a small selector trait (`get(&str)`/`default() ->
Arc<dyn LlmService>`) in `phoenix-core`; have `ModelRegistry` impl it;
change `ToolContext` to hold `Arc<dyn LlmSelector>` instead of
`Arc<ModelRegistry>`. Then `tools` depends only on core + `phoenix-terminal`.
Bounded but non-trivial: touches `ToolContext::new` (2 production callers +
~18 test builders), and requires confirming `LlmResponse`/`LlmError`/
`TokenChunk` are core-resident (errors live in `llm/error.rs`). Pair with the
small skills move. Rejected alternatives: moving the whole `llm` module to core
(drags reqwest/providers + the `api::ModelInfo` cycle into a "domain
vocabulary" crate); splitting `ToolContext` across crates (the trait ends up in
core anyway, for worse).

### Stage 3 status

Per-crate gating is mechanically proven by the `clippy -p` / `test -p` numbers
above; the dev.py wiring (`-p <changed-crate>` + `nextest -E 'rdeps(...)'`,
falling back to full when `phoenix-core` changes) is not yet implemented —
worth doing once ≥1 big module (e.g. `tools`) is its own crate, so a common
edit actually hits the fast path.
