# Normalize PR auto-fix LLM instructions

## Problem

Recent `pr_monitoring.rs` work introduced LLM-facing instruction text directly in the PR monitoring API layer. That bypasses the dedicated `crates/phoenix-core/src/llm_language.rs` translation layer, so future language variants can drift or miss this prompt entirely.

The same generated "address CI/review comments" instruction also includes ad-hoc push policy: `Do not push unless the user explicitly asks.` Phoenix should be largely agnostic about whether the agent pushes; that behavior should follow the user's existing guidance and the active mode/system instructions rather than a feature-specific prompt override.

## Plan

1. Add a language-aware helper in `crates/phoenix-core/src/llm_language.rs` for the PR auto-fix instruction, accepting the captured artifact path.
2. Replace the hardcoded `format!` string in `crates/phoenix-ide/src/api/pr_monitoring.rs` with the new helper.
3. Remove the explicit push/no-push instruction from the PR auto-fix prompt; keep the artifact as the source of truth and retain guidance to fix issues, run targeted tests, commit changes, and summarize.
4. Add/update tests so:
   - the PR auto-fix response message is produced through `llm_language.rs`,
   - the message references the artifact path and CI/review context,
   - the message does not contain feature-specific push policy.
5. Run the targeted Rust tests for PR monitoring/language prompt behavior, then `./dev.py check` if time allows.

## Acceptance criteria

- No LLM-facing PR auto-fix prompt prose remains hardcoded in `pr_monitoring.rs`.
- PR auto-fix prompt text lives in `crates/phoenix-core/src/llm_language.rs` with Phoenix-native and Caveman variants.
- The generated "address PR feedback" message does not instruct the agent whether to push.
- Existing PR context artifact behavior is unchanged.
