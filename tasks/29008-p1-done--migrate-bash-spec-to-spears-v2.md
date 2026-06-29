# Migrate bash spec to spEARS v2

## Goal

Use `specs/bash/` as the first concrete v1-to-v2 migration exemplar. Extract obvious design rationale from `specs/bash/design.md` into project ADRs, reconcile current behavior with `bash.allium` / `requirements.md` / `executive.md`, and reduce direct `design.md` authority references without changing product behavior.

## Scope

- Read `specs/bash/requirements.md`, `specs/bash/design.md`, `specs/bash/bash.allium`, `specs/bash/executive.md`, `specs/adrs/README.md`, and bash implementation comments under `crates/phoenix-tools/src/bash/`.
- Inventory `design.md` sections into v2 homes: requirements, Allium, ADR, executive, implementation-local comment, or legacy remainder.
- Add ADR(s) for clear bash design decisions, especially detached first-class handles and any output/process-lifetime rationale that is currently only prose.
- Update `specs/adrs/README.md` for new ADRs.
- Update bash spec/code references where safe to point at REQ IDs, Allium rules/entities, or ADRs.
- Leave `specs/bash/design.md` in place if any content remains unmigrated; add a legacy migration note rather than deleting it.

## Non-goals

- Do not change bash runtime behavior.
- Do not perform a repo-wide design-doc cleanup.
- Do not migrate unrelated core specs.

## Acceptance criteria

- Bash design rationale that was clearly ADR-worthy is represented in `specs/adrs/`.
- `specs/bash/design.md` clearly identifies what has moved and what remains legacy.
- Updated references do not point at removed/non-authoritative bash design sections.
- Relevant checks pass: `./dev.py check --lanes spec-shape,allium,spec-anchors,fast` and any focused tests touched by code comment/codegen changes.

## Phase 2 note

Created ADR-001 and ADR-002 to capture the migrated bash rationale about
first-class process-local handles / wait windows and watch-backed handle state
with snapshot shaping.
