# Retire bash design.md after spEARS v2 migration

## Goal

Finish the bash exemplar migration by removing `specs/bash/design.md`. Any remaining unique content must either have a spEARS v2 home (`requirements.md`, `bash.allium`, `executive.md`, or `specs/adrs/`) or be deleted as non-living migration/testing scaffolding.

## Scope

- Add ADR-003 for bash process cleanup via subreaper plus shutdown kill-tree, including the boundary between explicit user kill escalation and forceful shutdown cleanup.
- Verify hard-delete cascade, ring-buffer/output behavior, wait_seconds=0, command-safety relocation, and Explore sandbox behavior are captured in v2 artifacts or active follow-up tasks.
- Update references away from `specs/bash/design.md`.
- Delete `specs/bash/design.md`.

## Non-goals

- Do not change bash runtime behavior.
- Do not migrate unrelated design docs.
- Do not keep a design.md stub for posterity.

## Acceptance criteria

- `specs/bash/design.md` is removed.
- No in-repo reference treats `specs/bash/design.md` as current authority.
- ADR-003 is indexed in `specs/adrs/README.md` and passes spec-shape validation.
- Relevant checks pass.
