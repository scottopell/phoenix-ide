# spEARS Agent Rules

Phoenix agents use spEARS v2.

This file is intentionally short so it cannot drift from the packaged skill.
For full methodology details, use the built-in `spears` skill. For the
repository's temporary legacy-spec migration workflow, use
`skills/spears-v2-migrate/SKILL.md`.

## Current artifact rules

- `requirements.md` contains timeless user need and REQ-* requirements.
- `.allium` is optional precision-on-demand for state machines, lifecycles,
  multi-step operations, partial failure, and cross-boundary contracts.
- `specs/adrs/` is the shared project-level ADR chain for decision history.
- `executive.md` is the status/current-reality and verification-coverage artifact.
- New specs do not create a required `design.md`.
- Legacy `design.md` files are migrated into v2 homes and deleted; do not keep
  them as historical stubs.

## Agent workflow

1. Use the `spears` skill for new spec authoring.
2. Use `skills/spears-v2-migrate/SKILL.md` for retiring legacy design docs.
3. Resolve Allium open questions before merging spec changes.
4. Run relevant checks, at minimum:

```bash
./dev.py check --lanes spec-shape,allium,spec-anchors,fast
```
