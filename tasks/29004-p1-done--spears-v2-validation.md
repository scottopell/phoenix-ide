# Update Phoenix spec validation for spEARS v2

## Goal

Make Phoenix's deterministic spec-authoring checks understand the spEARS v2 artifact model so new work is validated against the target shape instead of the legacy v1 three-document shape.

## Background

Phoenix now teaches spEARS v2 for new specs:

- `requirements.md` is timeless and normative.
- `.allium` is optional precision-on-demand and normative when present.
- `executive.md` is the status/current-reality exception.
- `specs/adrs/` is one shared project-level decision chain.
- `design.md` is no longer required for new specs; existing `specs/*/design.md` files are legacy source material until decomposed.

Any validation, authoring checklist, lint, or audit script that still assumes `requirements.md` + `design.md` + `executive.md` as the required current shape will keep pulling agents back to v1.

## Scope

1. Inventory deterministic validation entry points:
   - `./dev.py check` lanes that validate specs or anchors.
   - `./dev.py audit-specs` behavior.
   - scripts under `scripts/`, `dev/`, or spec-authoring helpers that mention required spec files.
   - `specs/AUTHORING.md` pre-flight checklist.
2. Change validation rules for **new/current v2 specs**:
   - do not require `design.md`.
   - recognize `specs/adrs/README.md`, `_TEMPLATE.md`, and numbered ADR files as the shared ADR chain.
   - validate ADR filename shape, sequential numbering, README index rows, and `Affects:` presence.
   - continue validating `requirements.md`, `.allium`, and `executive.md` according to their artifact roles.
3. Preserve safe handling for the legacy corpus:
   - existing `specs/*/design.md` files should be reported/classified as legacy v1 artifacts, not validation failures.
   - validation should catch *new* v1-pattern instructions where feasible without blocking the existing tree.
4. Update tests or add regression coverage for:
   - a v2 spec with no `design.md` passes.
   - ADR chain files are discoverable and validated.
   - an existing legacy `design.md` does not fail the whole repo.

## Non-goals

- Do not decompose or delete legacy `design.md` files.
- Do not rewrite feature specs beyond validation/checklist references needed for v2.
- Do not change Allium grammar or Allium validation semantics.

## Acceptance criteria

- `./dev.py check` passes with the current mixed v1/v2 corpus.
- A new spec directory with `requirements.md` + `executive.md` and no `design.md` is accepted by the relevant validation path.
- `specs/adrs/` is validated for template/index/numbered ADR shape.
- `specs/AUTHORING.md` no longer presents `design.md` as required current guidance.
