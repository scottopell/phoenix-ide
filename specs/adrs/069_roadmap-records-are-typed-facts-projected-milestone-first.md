# ADR-069: Roadmap records are typed facts projected milestone-first

- **Status:** Accepted
- **Date:** 2026-09-27
- **Affects:** REQ-ROADMAP-001 through REQ-ROADMAP-014

## Context

The v1 roadmap Issue projected one row per workstream. The latest comment for a
workstream replaced the whole row, and its state, blockers, and context were
free text. Every update therefore restated every claim, so stale claims were
copied forward: cleared holds reappeared, merged and deployed work stayed listed
as unfinished, closed PRs kept "ready to merge", and new commissions were folded
into existing rows to stay under the row cap. The only freshness signal was one
Issue-wide marker that any comment advanced.

Contributors are now several harnesses on different machines — a primary and a
worker Phoenix instance, occasionally Codex and Claude Code — all posting under
the same GitHub account. Many v1 references (conversation URLs, work-scope
identifiers, runtime reports) resolve only on the machine that produced them.

## Options considered

1. **Keep v1 rows and add discipline** — cheapest; does not address copying
   claims forward, since every update must still restate the whole row.
2. **A replicated CRDT document** — convergent under concurrent edits, but
   GitHub already totally orders comments and one reducer folds them, so
   replica convergence is not the missing property.
3. **Typed facts in the same comment log, each kind with its own merge rule** —
   outcomes, milestones, gates, decisions, evidence, and status; gates clear
   permanently, decisions supersede explicitly, evidence is keyed by the
   version it describes, and PR state is derived from GitHub.
4. **Outcomes as GitHub Issues with native milestones** — reuses GitHub
   objects, but spreads state across many Issues and still needs typed
   evidence and gates.

## Decision

Option 3. The problem was that posting order decided truth; per-kind merge
rules make order matter only where it is meaningful (supersession, a newer
status) and bind other facts to what they describe (a PR head, a gate
identifier). The coordinator curates outcomes, milestones, and priority;
workers contribute evidence, status, and technical gates; user decisions are
recorded with the user's quoted words. The projection is milestone-first and
shows per-outcome freshness.

The cutover is hard: v1 records are rejected, and the roadmap moves to a new
Issue seeded before the repository variable is switched, so no projection mixes
the two models.

## Consequences

- **Positive:** cleared holds cannot resurrect; late results for old heads
  cannot qualify new heads; merge state cannot go stale; split web/native
  delivery is representable; optional outcomes cannot block milestones.
- **Negative:** posting requires more structure than v1, and the coordinator
  must record user decisions promptly for the projection to be right. Roles are
  declared, not authenticated.
- **Neutral:** v1 history stays readable on the closed v1 Issue.

## References

- `specs/roadmap/requirements.md`
- `scripts/roadmap-issue-reducer.mjs` — `reduceComments`, `surfaceDelivery`,
  `renderRoadmap`
