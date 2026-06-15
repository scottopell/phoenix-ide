# Workspace Cargo and dead-code allowance cleanup

## Goal

Perform a YAGNI-focused Rust workspace cleanup in four logical commits, targeting stale crate dependencies/features and broad or future-work `allow(dead_code)` suppressions.

## Commit 1: remove stale direct dependencies from `phoenix-ide`

Audit and remove direct dependencies from `crates/phoenix-ide/Cargo.toml` that appear to have moved into `phoenix-tools` and have no direct usage in `crates/phoenix-ide/src`:

- `similar`
- `globset`
- `regex`
- `chromiumoxide`
- `brush-parser`
- `unicode-security`

Validate with `./dev.py check` or the narrowest available Rust/check lane. If any dependency is still required by hidden build/test code, restore it and document why.

## Commit 2: remove or narrow broad `allow(dead_code)` suppressions

Start with the highest-smell broad suppressions:

- `crates/phoenix-tools/src/browser/session.rs`
- `crates/phoenix-ide/src/runtime.rs`
- `crates/phoenix-ide/src/llm/registry.rs`
- `crates/phoenix-tools/src/bash/handle.rs`
- `crates/phoenix-tools/src/bash/ring.rs`

Remove file-wide allowances where possible. Let compiler/clippy identify exact unused items. For each reported item:

- delete it if genuinely vestigial,
- gate it with `#[cfg(test)]` if test-only,
- or replace broad allowances with narrow, locally justified ones when the field/function exists solely for lifetime retention or public API compatibility.

Avoid retaining comments that describe future phases/tasks instead of local facts.

## Commit 3: clean dormant state-machine outcome architecture

Audit `crates/phoenix-state-machine/src/outcome.rs`, especially future-work allowances around:

- `LlmOutcome::Cancelled`
- `AbortReason::Timeout`
- `AbortReason::ParentCancelled`
- `PersistOutcome`
- `SpawnOutcome`
- `EffectOutcome::SubAgent`
- `EffectOutcome::Persist`

Either wire the typed outcome path if it is actually ready, or delete dormant variants/types until they have production consumers. Prefer reducing representable-but-unhandled states over keeping architectural placeholders.

## Commit 4: trim overbroad Tokio features

Narrow `tokio = { features = ["full"] }` in library crates where compiler-guided trimming is practical:

- start with `crates/phoenix-tools/Cargo.toml`
- then `crates/phoenix-terminal/Cargo.toml`
- consider `crates/phoenix-ide/Cargo.toml` only after the library crates are stable

Use actual compile errors to determine required feature sets. Expected production features for `phoenix-tools` are likely around `fs`, `io-util`, `process`, `rt`, `sync`, `time`, and `macros`; tests may require `net`, `rt-multi-thread`, and `test-util`.

## Validation

- Run `./dev.py check` before finalizing.
- If the full check is too slow during intermediate commits, run targeted Rust checks first, then full check at the end.
- Keep commits logical and reversible.

## Non-goals

- Do not redesign MCP, browser tooling, or terminal architecture.
- Do not remove live dependencies simply because they are heavy.
- Do not keep broad `allow(dead_code)` as a substitute for deciding whether code is test-only, public API, lifetime-retention, or vestigial.
