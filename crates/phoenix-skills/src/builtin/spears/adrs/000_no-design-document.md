# ADR-000: spEARS Has No Living Design Document

- **Status:** Accepted
- **Date:** 2026-06-28
- **Affects:** methodology-level — the entire spEARS artifact model

## Context

A specification methodology has to record how a system is designed.
The obvious home is a design document: a prose file describing the current
architecture, kept up to date as the system evolves.
Every prior version of spEARS shipped one (`design.md`).

In practice that document loses the race with the code.
Prose has no checker; nothing fails when it falls out of sync, so it drifts —
and a drifted design document is worse than none, because it is confidently
wrong. Keeping it current is a permanent discipline tax that no team pays
forever. The root cause is a medium mismatch: prose is inherently point-in-time,
and a living design document asks it to be an always-current mirror of running
code, a job prose cannot hold honestly.
(Full exposition:
[`../references/design-philosophy.md`](../references/design-philosophy.md).)

## Options considered

1. **Keep a living `design.md`, enforced by discipline.** Familiar, and one
   place to look for “how it works.”
   But it rots the moment the discipline lapses, and the rot is invisible until
   it bites someone.
2. **Drop design documentation entirely; let the code be the design.** No rot,
   no tax. But design *intent* — why it is built this way — lives nowhere, and is
   reinvented (often wrongly) on every change.
3. **Decompose the design document by relationship to time.** Current behavior
   goes to a checkable DSL (Allium) that cannot silently drift; the *why* goes
   to a chain of frozen, point-in-time ADRs; timeless user need stays in
   `requirements.md`; status stays in `executive.md`.

## Decision

Adopt option 3. spEARS has no living design document.
“How it behaves now” is captured, on demand, in Allium, where a checker keeps it
honest.
“Why we chose it” is captured in ADRs like this one, frozen at the moment
of decision. Timeless user need stays in `requirements.md`; current status stays
in `executive.md`. No single artifact is asked to be a timeless, hand-maintained
description of current design, because that is the one thing the medium cannot
honestly be.

## Consequences

- **Positive:** no staleness tax on a design doc; behavioral truth is
  machine-checkable via Allium; design intent accumulates as a greppable ADR
  chain whose *pattern* reveals the project’s priorities more honestly than any
  stated list of principles.
- **Negative:** Allium is a heavier tool, applied only where behavioral
  complexity warrants it — a judgment call, not a default.
  Design rationale is spread across many ADRs rather than one page, so
  understanding “the whole design” means reading a chain, not a document.
- **Neutral:** spEARS’s founding decision is itself a spEARS ADR — the method’s
  first artifact is an instance of the method, which is the strongest evidence
  the form carries its weight.

## References

- [`../references/design-philosophy.md`](../references/design-philosophy.md) —
  the full exposition this decision points back to.
- [`_TEMPLATE.md`](_TEMPLATE.md) — the ADR format this record exemplifies.
- The four-artifact model in [`../SKILL.md`](../SKILL.md) that this decision
  establishes.
