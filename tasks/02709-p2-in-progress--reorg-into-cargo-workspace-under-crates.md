---
created: 2026-05-08
priority: p2
status: in-progress
artifact: pending
---

# reorg-into-cargo-workspace-under-crates

## Plan

# Reorganize into a Cargo workspace under `crates/`

**Base off latest `origin/main`.** Browser scope has grown since this was scoped; this task is structural and shouldn't conflict with browser changes — it moves files but doesn't touch their contents (other than `tls_certs.rs` becoming a separate crate's library).

## Summary

Convert the single-package layout into a clean Cargo workspace. All Rust source moves under `crates/`. Two members for now:

- `crates/phoenix-ide/` — everything currently in `src/` (the main server, monitor binary, all modules)
- `crates/phoenix-tls/` — the TLS certificate utility, with its own minimal dep tree

Result: `./dev.py tls` no longer drags chromiumoxide / tokio / sqlx / etc. into its dep graph; cold-target compiles in fresh worktrees go from minutes to seconds. The new workspace shape also makes future per-module splits (e.g. `phoenix-llm`, `phoenix-state-machine`) routine refactors rather than structural ones.

## Final layout

```
phoenix-ide/                  ← repo root
├── Cargo.toml                ← virtual workspace manifest only (no [package])
├── Cargo.lock                ← shared
├── crates/
│   ├── phoenix-ide/
│   │   ├── Cargo.toml        ← contents of current root Cargo.toml's [package], [dependencies], [dev-dependencies], [[bin]]s (minus phoenix-tls)
│   │   └── src/              ← contents of current src/
│   └── phoenix-tls/
│       ├── Cargo.toml        ← rcgen + time, single [[bin]]
│       └── src/
│           ├── lib.rs        ← contents of current src/tls_certs.rs (with pub APIs)
│           └── main.rs       ← contents of current src/bin/tls.rs (using phoenix_tls::)
├── ui/                       ← unchanged
├── specs/, tasks/, etc.      ← unchanged
```

## Rationale recap

Picked option (C) from the panel debate: smallest surgical fix to the actual stated pain (cold `./dev.py tls`), no architectural commitment, doesn't preclude future feature-flag work on chromiumoxide. Per the user's review note, no `members = ["."]` — clean reorg with all Rust source under `crates/`.

Decision on granularity: **two crates only.** The top-level modules (`api`, `chain_qa`, `db`, `llm`, `runtime`, `state_machine`, `terminal`, `tools`, etc.) are tightly cross-coupled and don't have clean cut points. Pulling out subsystems like `phoenix-llm` is its own task with its own API-boundary work; doing it speculatively here is exactly the scope creep the panel warned against. The workspace shape this task introduces makes any future split a routine refactor.

## What to do

### 1. Move main package source

```
mkdir -p crates/phoenix-ide
git mv src crates/phoenix-ide/src
```

`git mv` preserves rename history. No file contents change in this step.

### 2. Create `crates/phoenix-ide/Cargo.toml`

Contains everything currently in the root `Cargo.toml` *except*:
- The `[[bin]] name = "phoenix-tls"` block (gets dropped — moves to the new crate)
- `rcgen` and `time` from `[dependencies]` (only used by `tls_certs.rs`, which is leaving)
- The `[workspace]` table (lives at the root)
- The `[profile.*]` and `[workspace.lints]` blocks if we move them workspace-up (see step 4)

Add as a new dependency:
```toml
phoenix-tls = { path = "../phoenix-tls" }
```

`[[bin]]` paths inside `crates/phoenix-ide/Cargo.toml` stay relative to the crate, so `path = "src/bin/monitor.rs"` (no change needed).

### 3. Create `crates/phoenix-tls/`

`crates/phoenix-tls/Cargo.toml`:
```toml
[package]
name = "phoenix-tls"
version = "0.1.0"
edition = "2021"
rust-version = "1.94"

[lib]
path = "src/lib.rs"

[[bin]]
name = "phoenix-tls"
path = "src/main.rs"

[dependencies]
rcgen = { version = "0.14", features = ["x509-parser"] }
time = "0.3"
```

`crates/phoenix-tls/src/lib.rs` — contents of current `src/tls_certs.rs`. Change `pub(crate)` to `pub` on the public surface (`ensure_ca`, `issue_leaf`, `CertKeyPaths`, `CaPaths`, `ca_paths`). Internal helpers (`write_ca`, `write_leaf`, `write_pem`, `validity_window`, `PemKind`, the constants) stay private.

`crates/phoenix-tls/src/main.rs` — contents of current `src/bin/tls.rs`, with the `#[path = "../tls_certs.rs"] mod tls_certs;` hack replaced by `use phoenix_tls::{ensure_ca, issue_leaf};` and the call sites updated accordingly.

### 4. Replace root `Cargo.toml` with a virtual workspace manifest

```toml
[workspace]
resolver = "2"
members = ["crates/phoenix-ide", "crates/phoenix-tls"]

[workspace.lints.rust]
unused_extern_crates = "deny"
unused_allocation = "deny"
unused_assignments = "deny"
unused_comparisons = "deny"

[workspace.lints.clippy]
pedantic = { level = "deny", priority = -1 }
float_cmp = "deny"
manual_memcpy = "deny"
redundant_allocation = "deny"
rc_buffer = "deny"
unnecessary_to_owned = "deny"
dbg_macro = "deny"
mod_module_files = "deny"
string_slice = "deny"

[profile.release]
debug = true

[profile.dev.package.chromiumoxide]
opt-level = 3
[profile.dev.package.tokio]
opt-level = 3
[profile.dev.package.reqwest]
opt-level = 3
# (... and the other heavy deps the existing Cargo.toml lists)
```

Then in `crates/phoenix-ide/Cargo.toml` and `crates/phoenix-tls/Cargo.toml`, opt into the workspace lints:
```toml
[lints]
workspace = true
```

This consolidates lints in one place and applies them to both members.

### 5. Update intra-package references

- `crates/phoenix-ide/src/main.rs` (formerly `src/main.rs`) — remove `mod tls_certs;` (line 21).
- `crates/phoenix-ide/src/tls.rs` line 254 — change `crate::tls_certs::issue_leaf(dir, ...)` to `phoenix_tls::issue_leaf(dir, ...)`.
- Delete `crates/phoenix-ide/src/tls_certs.rs` (moved into the new crate as `lib.rs`).
- Delete `crates/phoenix-ide/src/bin/tls.rs` (moved into the new crate as `main.rs`).

### 6. Verify `dev.py` still works

Most invocations should keep working unchanged because Cargo resolves `--bin <name>` across the whole workspace from the root. Spots to check:

- `dev.py:765` — `cargo run --quiet --bin phoenix-tls -- <args>`: should keep working. If Cargo gripes, switch to `cargo run --quiet -p phoenix-tls --bin phoenix-tls -- <args>`.
- Wherever `dev.py` invokes the main server binary: confirm the binary name didn't change. Currently the package name is `phoenix_ide` so the implicit main binary is `phoenix_ide` (underscore). The new `crates/phoenix-ide/` package keeps that name. If `dev.py` references a path like `target/debug/phoenix_ide`, that path is unchanged. If it references `target/debug/phoenix-ide` (hyphen), there must be an explicit `[[bin]] name = "phoenix-ide"` somewhere (preserve it on the move).
- `cargo` from the workspace root continues to work; `cargo` from inside a member crate works for that member.

The other `phoenix_tls` / `phoenix-tls.json` references in `dev.py` (lines 430, 484, 553, 859, 883, 890, 911, 916, 922, 978-982) are unrelated — they refer to a Python boolean variable and a metadata JSON filename, not the binary.

### 7. Anything else that hardcodes paths

Sweep for path assumptions and update if needed:
- `.gitignore` — `target/` at the root still works (workspace shares one `target/`).
- `rust-toolchain.toml` if present — stays at the root.
- `clippy.toml`, `rustfmt.toml` if present — stay at the root, apply workspace-wide.
- CI configs (search for `src/` or specific paths in any `.yml`/`.yaml` under the repo).
- `./dev.py codegen` — `cargo test` triggers ts-rs export to `../ui/src/generated/`. The `export_to` path in `#[ts(export_to = "...")]` attributes is relative to the source file. Since `src/api/wire.rs` becomes `crates/phoenix-ide/src/api/wire.rs`, the existing `"../ui/src/generated/"` resolves to `crates/phoenix-ide/ui/src/generated/` — **wrong**. These need updating to `"../../../ui/src/generated/"` (one extra `..` for each new directory level).

This last point is the only non-trivial sweep. Acceptance criterion 4 below catches it.

## Acceptance criteria

1. `./dev.py up` builds the main server unchanged and starts cleanly.
2. `./dev.py tls` issues a cert successfully (smoke-test by running it after `cargo clean` in a scratch worktree).
3. `./dev.py check` passes (clippy + fmt + tests + codegen-stale guard + task validation).
4. **TS codegen still emits to the right place.** After `./dev.py codegen`, `git diff ui/src/generated/` is empty (i.e. files exist where the build expects them). If they emit to a wrong path, fix the `#[ts(export_to = ...)]` attributes.
5. **The win is verified:** with a cold `target/`, `cargo build -p phoenix-tls` does NOT compile chromiumoxide. Verify by running `cargo build -p phoenix-tls -v 2>&1 | grep -E '(chromiumoxide|tokio|sqlx)'` and confirming no matches. Build should complete in under ~15 seconds on a cold target instead of minutes.
6. `cargo build --workspace` from the repo root builds everything.
7. `cargo run --bin phoenix-tls -- ca --dir /tmp/test-ca` works from the repo root (i.e. `dev.py:765`'s invocation pattern still resolves).
8. The main server binary name is unchanged (i.e. whatever path/name `dev.py` and any deploy script use to launch the server still resolves).
9. No new comments needed in code; the split is self-evident from the directory structure.

## What this does NOT do

- Does **not** touch `ToolContext`, `BrowserSessionManager`, or any browser code. The `chromiumoxide`-as-optional-feature work and the SessionManager-trait debate are deferred per the panel synthesis (panel landed 3-1 against the trait abstraction; chromiumoxide-as-feature is deferred until there's a concrete "no-browser build" user demand).
- Does **not** introduce a Cargo feature flag for browser tools.
- Does **not** split the main package into multiple subsystem crates (`phoenix-llm`, `phoenix-state-machine`, etc.). The workspace shape this task creates makes those routine future refactors.
- Does **not** rename the main package from `phoenix_ide` to anything else.

## Risks

- **TS codegen path attributes break silently.** The `export_to` paths in `#[ts(export_to = "...")]` attributes resolve relative to the source file. Moving source under `crates/phoenix-ide/src/` adds two directory levels. Caught by acceptance criterion 4.
- **A binary name lookup somewhere uses an unexpected path.** Mitigated by acceptance criteria 1, 2, 7, 8 covering every known invocation path.
- **`Cargo.lock` regeneration.** The new workspace will rebuild the lockfile. Diff should be additive (new `phoenix-tls` entry, no version drift). Verify the generated lockfile doesn't pick up unexpected new versions of existing deps.
- **Browser scope drift in `main` since scoping.** The user flagged that browser code has grown. This task moves files in `src/` wholesale (`git mv src crates/phoenix-ide/src`), so the rebase against `origin/main` happens before the move. Resolution should be standard merge mechanics with no special browser knowledge needed.

## Progress

