A module-wide `#![allow(dead_code)]` at the top of `runtime.rs` silences
dead-code detection for the entire runtime subsystem -- the crate's largest --
and its justifying comment is false.

## Verified location
- crates/phoenix-ide/src/runtime.rs:7 --
  `#![allow(dead_code)] // browser_sessions() will be used when browser cleanup is wired up`
  The `#![...]` inner attribute applies to the whole `runtime` module tree:
  executor.rs (~5k lines), recovery.rs, traits.rs, testing.rs, etc.

## Why egregious
1. The comment is false. `browser_sessions()` (runtime.rs:889) is already
   wired up and called -- api/browser_view.rs:53 and api/handlers.rs:348. The
   actually-dead sibling method is `platform()` at runtime.rs:884, which the
   comment does not mention.
2. It silences detection, not a known item. Any genuinely-dead code later
   written into executor.rs / recovery.rs / etc. produces no warning -- the
   "capability gaps are silenced" anti-pattern applied to the compiler's own
   dead-code lint.
3. The codebase demonstrably knows the correct (targeted) pattern: the same
   file has 4 targeted `#[allow(dead_code)]` with per-item justifications
   (runtime.rs:54, 906, 912, 1764); traits.rs has 3 (107, 176, 410);
   executor.rs has 1 (2179); testing.rs has 8. The blanket allow makes all 16
   redundant while hiding everything else.

## Verified scope of what it hides (measured by deleting line 7)
- `cargo check -p phoenix_ide` (non-test build): 0 warnings.
- `cargo test --no-run -p phoenix_ide` (test build): exactly 8 dead-code
  warnings:
  - `platform` method -- runtime.rs:884
  - `content_blocks_strategy` fn -- runtime/recovery.rs:579 (proptest helper)
  - `get_message_by_id` -- runtime/traits.rs:76 (trait decl + impls), runtime/testing.rs:501
  - `get_conversation_mode` -- runtime/traits.rs:116 (trait decl + impls), runtime/testing.rs:583
  - `builtin_only` assoc fn -- runtime/traits.rs:663
  - fields `llm` and `tools` -- runtime/testing.rs (test harness struct ~line 656)
  - `conv_id` -- runtime/testing.rs:687
  - `send_cancel` -- runtime/testing.rs:767

## Fix direction
Delete `#![allow(dead_code)]` from runtime.rs:7. Triage the 8 surfaced items:
most are test scaffolding (testing.rs helpers, the recovery.rs proptest
strategy) and should get targeted `#[allow(dead_code)]` or `#[cfg(test)]`
placement; `platform()` and the `builtin_only` / `get_message_by_id` /
`get_conversation_mode` trait-API items should be assessed for genuine deletion
vs. a targeted allow with an honest justification. Verify with
`cargo test --no-run` and `./dev.py check`. NOT a trivial fix -- the 8 items
need per-item judgment, which is why the code-correctness audit filed this
rather than fixing it directly.

## Related
- 08004-p3-done (clean-up-compiler-warnings)
