# Clean up code and generated references to legacy design docs

## Goal

Remove or replace source-code and generated-documentation comments that treat `specs/*/design.md` as the current authority after spEARS v2 adoption.

## Background

The bootstrap inventory found design-doc citations in Rust doc comments, inline comments, generated TypeScript, UI tests, Allium comments, requirements/executive docs, and historical task files. Some are harmless historical references; others route future maintainers to the wrong artifact.

## Scope

1. Inventory non-task references with `rg "design.md|specs/.*/design.md"` across:
   - `crates/`
   - `ui/src/`
   - `specs/`
   - generated files under `ui/src/generated/`
2. For each reference, choose the right replacement:
   - REQ ID for user-visible requirement authority.
   - Allium entity/rule/transition name for precise current behavior.
   - ADR number for design rationale or tradeoff history.
   - Rust/TypeScript symbol name for implementation-local facts.
   - Legacy note only when the target design doc has not yet been decomposed.
3. Update comments in source files and regenerate generated files when source doc comments feed codegen.
4. Leave historical completed task files alone unless a task template or ready task would mislead future work.
5. Coordinate with task 29005: if a reference points to content that still exists only in a legacy design doc, either migrate that content first or leave a clearly marked legacy reference.

## High-priority known reference clusters

- `crates/phoenix-ide/src/api/handlers.rs` → `bedrock/design.md` FM-7.
- `crates/phoenix-ide/src/chain_runtime.rs`, `crates/phoenix-ide/src/api/chains.rs`, `ui/src/utils/chains.ts`, and related tests/generated docs → `chains/design.md`.
- `crates/phoenix-terminal/src/*`, `crates/phoenix-tools/src/tmux*`, generated tmux types → terminal/tmux design docs.
- Allium comments in `llm`, `wake-contracts`, `working-phase-visibility`, `terminal-panel`, `stale-tool-results`, and `messagelist-render-units` that cite design prose for current behavior.

## Non-goals

- Do not delete design docs by itself; this task cleans references.
- Do not rewrite completed historical tasks except where they are used as active templates or guidance.
- Do not change product behavior.

## Acceptance criteria

- No source or generated comment points to a removed/non-authoritative design-doc section.
- Remaining `design.md` references are either historical or explicitly marked as legacy pending decomposition.
- Codegen is refreshed if generated files change.
- `./dev.py check` passes.
