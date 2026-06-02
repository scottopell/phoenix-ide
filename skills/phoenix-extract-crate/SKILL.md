---
name: phoenix-extract-crate
description: Procedure and invariants for splitting a module out of the phoenix-ide monolith into its own acyclic workspace crate (and for the per-crate `./dev.py check` gating that pays for it). Use when extracting a crate, breaking a dependency cycle to enable extraction, sinking a shared type into phoenix-core, or rebasing in-flight extraction work onto a moved main.
---

# Extracting a Crate from phoenix-ide

Splitting a module into its own library crate lets Cargo recompile only the
changed crate + its dependents, and lets `./dev.py check` lint/test only the
affected crates. The win is **proportional to how much code lives in
right-sized crates** — a one-line edit in an extracted crate checks in ~3–7s
vs ~40–45s workspace-wide.

## The governing principle: types sink, behavior floats

`phoenix-core` is the **acyclic base crate**: it holds the shared, serializable
*domain vocabulary* (`domain::{llm_types, db_schema, sm_state, sm_event,
bash_types, tool_wire, patch_types, kill_signal, …}` + leaves `llm_language`,
`task_source`, `work_scope`, `platform`) and narrow service traits
(`CompletionService`, `LlmSelector`). **Logic stays up** in the crate that owns
it and depends *down* onto core. Every other crate depends only on core and on
crates strictly below it. The graph must be a DAG — verify after every step.

When two crates need the same type, the type sinks to core; the *behavior*
(parsing, I/O, anything pulling `reqwest`/`axum`/`sqlx`) stays in the crate
that owns it. Example: `QuotaDetails`/`RateLimitWindow`/`CreditsSnapshot` are
pure data → core; the `reqwest::HeaderMap` parsing fns stay in
`llm/rate_limit.rs` and reference the sunk types.

**Move-down, re-export-up.** In the parent that used to own the module, replace
`mod <name>;` with `use phoenix_<name> as <name>;` (or `pub use`) so existing
`crate::<name>::…` call sites resolve unchanged. Defer call-site rewrites.

## Before extracting: break wrong-way edges first

A module can only be lifted cleanly once it points *down*. Find upward/sibling
edges with `rg "crate::(api|runtime|llm|db|tools|skills|system_prompt)" crates/phoenix-ide/src/<module>`.
For each:

- **Shared pure-data type** referenced upward/sideways → sink it to
  `phoenix-core::domain`, re-export from the original location.
- **Concrete dependency on a heavy service** (e.g. `ToolContext` held
  `Arc<ModelRegistry>`, dragging in the 11k-LOC `llm` module) → invert behind a
  narrow trait in core (`Arc<dyn LlmSelector>`), impl the trait on the concrete
  type via an adapter. Do **not** move the heavy module into core — that drags
  its deps (reqwest, providers) and cycles into the "vocabulary" crate.
- **Misfiled glue** (e.g. `terminal/ws.rs` used `AppState`/`RuntimeManager`) →
  relocate it *up* into the layer it actually belongs to (`api`), leaving the
  crate's *core* (tmux, line streams) depending only on `phoenix-core`.
- **Doc-link `///` edges** → not compile edges; downgrade to plain text or
  intra-doc links.

Extract bottom-up so each new crate's deps are already crates or core.

## Per-extraction recipe

1. `git mv` the module's files into `crates/phoenix-<name>/src/` (`foo.rs` +
   `foo/`, never `foo/mod.rs` — clippy-enforced). `lib.rs` is the old `<module>.rs`.
2. Create `Cargo.toml`: copy an existing leaf crate's shape (`edition = "2021"`,
   `rust-version = "1.94"`, `[lints] workspace = true`). Add **only** the
   external deps the moved files actually use — derive them from `use`
   statements *and* inline paths (`uuid::`, `tracing::`, `taskmd_core::` are
   easy to miss because they aren't always in a `use`).
3. Add to workspace `members` (alphabetical) and as a path dep of the crates
   that need it.
4. Move-down/re-export-up in the parent (see above).
5. Rewrite intra-moved-file paths: `crate::<module>::X` → `crate::X`;
   `crate::<other-extracted>::Y` → `phoenix_<other>::Y` or
   `phoenix_core::domain::…`.
6. Validate (all must pass — see checklist).
7. Commit (one extraction = one commit). Push when the unit is complete.

## Known gotchas (expect these every time)

- **Cross-crate `#[cfg(test)]` is invisible.** A test-only helper in crate X
  used by crate Y's tests must be gated
  `#[cfg(any(test, feature = "test-support"))]`; X exposes a `test-support`
  feature; Y enables it via dev-dependencies (off in production builds). See
  phoenix-core's `ContentBlock::tool_use`, phoenix-db's `create_conversation`.
- **New public-API clippy surface.** Promoting binary-private code to library
  API fires pedantic lints the binary hid: `must_use_candidate`,
  `missing_errors_doc`. Auto-fix the first with
  `cargo clippy --fix -p phoenix-<name> --lib --allow-dirty`; add a one-line
  `# Errors` (and `# Panics` where `.expect()`/`unreachable!()` guard
  invariants) doc per flagged `pub fn`. **Fix, don't `#[allow]`.**
- **ts-rs `export_to` is manifest-dir-relative**, and all `crates/*` sit at the
  same depth, so `"../../../ui/src/generated/"` resolves unchanged after a
  move — but the new crate needs the `ts-rs` dep and the generated TS must come
  out **byte-identical** (the codegen-stale guard enforces this).
- **`cargo fmt` after `clippy --fix`** — the fixer leaves trailing whitespace.
- **Inherent impls can't split across crates.** If a sinking type has an
  inherent impl that needs a heavy dep, you cannot move the type while leaving
  that impl behind. Either the impl is pure (move it too) or the type can't
  sink — stop and reconsider.

## Validation checklist (every extraction)

```bash
cargo check -p phoenix-<name> && cargo check -p phoenix_ide
cargo clippy --workspace -- -D warnings          # default targets, matches ./dev.py check
cargo fmt --check
PHOENIX_SKIP_BROWSER_TESTS=1 PHOENIX_SKIP_NETWORK_TESTS=1 cargo test --workspace
cargo test --workspace export_bindings           # then:
git status --porcelain -- ui/src/generated/      # MUST be empty (no codegen drift)
```

Acyclicity (production code only — doc-links are fine):

```bash
# the new crate must not reach back up
rg "crate::(api|runtime|llm|db|tools|skills|system_prompt|chain_runtime|chain_qa)" crates/phoenix-<name>/src/
# a true leaf reaches nothing in the workspace
cargo metadata --no-deps --format-version 1   # inspect the crate's workspace deps
```

The authoritative gate is `PHOENIX_CHECK_ALL=1 ./dev.py check` — **all 17 lanes
green**. Browser/Chrome + network test failures are environmental, not
regressions; the `PHOENIX_SKIP_*` vars above isolate them.

## Stage 3: per-crate `./dev.py check` gating (the payoff)

Extraction alone barely moves check wall-clock — the lane still runs
workspace-wide. The gating in `dev.py` narrows `cargo clippy`/`cargo test` to
the changed crate(s) **+ their transitive reverse-dependency closure** (computed
live from `cargo metadata`, threaded as `-p <crate>` flags). `rdeps`, not
`deps`: a change can break dependents, so you test upward.

**Fail-safe by construction — only narrows, never wrongly skips:**
- `phoenix-core` changed → full closure (everything depends on it).
- `Cargo.lock` / workspace `Cargo.toml` / `.cargo/` / `rust-toolchain` /
  `ui/src/generated/` changed → full.
- any unattributable path or a `cargo metadata` failure → full.
- `--all` flag **or** `PHOENIX_CHECK_ALL=1` → full (both escape hatches must
  force full in lockstep — a past bug had the env var skip lanes but not the
  rust-scope narrowing).
- **codegen is never narrowed** — the ts-rs export tests + staleness guard must
  see the whole generated tree.

## Rebasing in-flight extraction work onto a moved main

A crate split is many commits; `main` moves under you. If your base was
squash-merged, recover with `git rebase --onto origin/main <last-merged-commit>
<your-branch>` — replay only the post-merge commits. **Zero textual conflicts
does NOT mean it integrates**: code `main` added to the *monolith* after you
diverged gets swept into your *extracted* crate with monolith-era paths
(`crate::work_scope`, `crate::api::PrDisplayState`) that no longer resolve
cross-crate, and may need deps the crate's `Cargo.toml` lacks (e.g. `serde`).
Always run a full `PHOENIX_CHECK_ALL=1 ./dev.py check` on the rebased branch and
fix the integration drift with the same sink/repoint recipe — a clean rebase is
necessary, not sufficient.
