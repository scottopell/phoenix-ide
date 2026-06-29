# Decompose high-risk core legacy design docs into spEARS v2 homes

## Goal

Migrate the most authority-bearing legacy design docs out of the v1 `design.md` role and into spEARS v2 homes without losing behavioral rules or rationale.

## Target specs

Prioritize these because code comments, Allium specs, generated docs, or active implementation paths cite their `design.md` files as current authority:

1. `specs/bedrock/`
2. `specs/projects/`
3. `specs/bash/`
4. `specs/terminal/` and `specs/terminal-panel/`
5. `specs/tmux-integration/`
6. `specs/mcp/`
7. `specs/chains/` if it remains heavily cited after the first six are handled

## Required method per spec

For each target spec:

1. Read `requirements.md`, `design.md`, `executive.md`, and any `.allium` files together.
2. Classify every load-bearing `design.md` section as one of:
   - requirement/user need → move or reconcile into `requirements.md`;
   - precise current behavior/invariant/transition/precondition → move or reconcile into `.allium`;
   - design rationale/option tradeoff → capture in `specs/adrs/NNN_<slug>.md`;
   - status/progress/current implementation reality → move or reconcile into `executive.md`;
   - obsolete or contradicted content → resolve explicitly before removal.
3. Update in-repo references that point at the migrated section:
   - prefer REQ IDs, Allium entity/rule names, ADR numbers, or symbol names.
   - regenerate generated files when Rust doc comments change.
4. Only delete or shrink `design.md` after extracted content has a verified v2 home.
   - If a design doc cannot be fully retired in one pass, leave a short legacy note listing remaining unmigrated sections.

## Non-goals

- Do not perform a mechanical delete of all design docs.
- Do not invent rationale where history cannot be reconstructed; if uncertain, either preserve the legacy note or ask for a decision.
- Do not broaden feature behavior while migrating docs.

## Acceptance criteria

- Each migrated domain has no lost requirements, behavioral rules, or rationale.
- Relevant `.allium` files validate after edits.
- Any new ADR is added to `specs/adrs/README.md`.
- Code/generated references no longer cite removed design-doc sections.
- `./dev.py check` passes.
