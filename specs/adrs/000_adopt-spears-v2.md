# ADR-000: Phoenix Adopts spEARS v2 for New Specification Work

- **Status:** Accepted
- **Date:** 2026-06-29
- **Affects:** methodology-level — Phoenix repository specification practice

## Context

Phoenix uses spEARS specs and Allium behavioral specs to keep product behavior,
agent guidance, and implementation traceable. The repository already contains a
large v1-style corpus with `requirements.md`, `design.md`, and `executive.md`
files, plus many `.allium` files. That corpus is useful, but the v1 artifact
model asks prose `design.md` files to describe current behavior and design
rationale at the same time.

spEARS v2 changes the methodology: timeless user need belongs in
`requirements.md`; precise current behavior belongs in `.allium` when the
feature warrants that weight; status belongs in `executive.md`; design rationale
belongs in one shared project-level ADR chain. Phoenix must teach this model to
agents for new work without pretending the existing v1 corpus has already been
fully migrated.

## Options considered

1. **Keep the v1 model for Phoenix until every existing spec is migrated.** This
   avoids a mixed corpus, but it keeps teaching new agents to create more living
   `design.md` files and expands the migration burden.
2. **Rewrite all existing specs to v2 in one pass.** This gives a clean tree, but
   it risks losing behavioral rules and design rationale because many design docs
   mix requirements, implementation notes, failure modes, and status history.
3. **Adopt v2 for new work and migrate the legacy corpus incrementally.** This
   stops new v1 drift, creates the shared ADR chain now, and leaves existing
   `design.md` files intact until each domain can be decomposed deliberately.

## Decision

Adopt option 3. Phoenix uses spEARS v2 for new specification work. New feature
specs use `requirements.md`, optional `.allium`, `executive.md`, and shared ADRs
under `specs/adrs/`; they do not create a required living `design.md`. Existing
`specs/*/design.md` files remain legacy v1 artifacts until follow-up work moves
their content into requirements, Allium, ADRs, or executive status as
appropriate.

The built-in `spears` skill and `AGENTS.md` guidance teach the v2 model so new
agent work follows the target methodology even while the repository contains a
legacy corpus.

## Consequences

- **Positive:** New specs stop adding living design documents; design rationale
  has a single project-level home; Phoenix can migrate high-risk specs with
  reviewable slices instead of a repo-wide rewrite.
- **Negative:** The repository carries a mixed v1/v2 corpus during migration.
  Agents must distinguish legacy `design.md` files from current guidance, and
  existing code comments that cite design docs need focused cleanup.
- **Neutral:** Existing v1 `design.md` files remain useful source material, but
  their content is not automatically authoritative in the v2 model; each domain
  needs deliberate decomposition.

## References

- `AGENTS.md` — repository guidance for new specification work.
- `crates/phoenix-skills/src/builtin/spears/SKILL.md` — built-in spEARS skill
  distributed with Phoenix.
- `tasks/29003-p1-in-progress--bootstrap-spears-v2-migration.md` — migration
  inventory and follow-up plan captured with this bootstrap slice.
