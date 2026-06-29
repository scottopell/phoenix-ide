# ADR-NNN: <The decision, as a short declarative — e.g. "Bash handles are first-class entities">

- **Status:** Proposed | Accepted | Superseded by ADR-NNN
- **Date:** YYYY-MM-DD
  <!-- add "(formalized from existing design)" if written retroactively from code -->
- **Affects:** REQ-XX-### (and/or Allium entities) — list every requirement this
  touches; “methodology-level” if it binds the whole project

<!-- An ADR is frozen once Accepted.
Do not rewrite it to match later reality — supersede it with a new ADR instead
(set this one's Status to "Superseded by ADR-NNN", and have the new ADR cite
this one in its Options considered).
The only edit an accepted ADR should receive is a cross-reference to a newer,
related ADR. ADRs live in ONE shared specs/adrs/ chain, numbered sequentially
across the whole project.
Scope is declared in Affects: above, never by directory location.
-->

## Context

*The forces at play.* What problem prompted a decision, what constraints bound
it, and why this was a genuine fork rather than an obvious call.
Write in the present tense, describing the situation as it stood when the
decision was made.

## Options considered

1. **<Option A>** — what it is; its advantages; its costs.
2. **<Option B>** — what it is; its advantages; its costs.
3. **<Option C>** — what it is; its advantages; its costs.

List the options that were *genuinely* on the table.
An ADR with one option is not recording a decision — it is decoration.
When this ADR supersedes another, the superseded decision belongs here, with why
it no longer holds.

## Decision

The option chosen, stated plainly, and the reasoning that made it win over the
others. This is the load-bearing section: be specific about *why this one*, not
just *which one*.

## Consequences

What the decision buys and what it costs.
The costs are what a future reader most needs — an ADR that lists only upsides
is not trustworthy.

- **Positive:** what becomes easier, safer, or possible.
- **Negative:** what becomes harder or more constrained; the price paid.
- **Neutral:** side effects that are neither win nor loss but worth knowing.

## References

- Related ADRs (by number), and the one this supersedes, if any.
- Code by **symbol name** — `SseBroadcaster::send_seq`, not `runtime.rs:529`
  (line numbers rot; symbols survive refactors).
- The feature spec(s) and external resources this decision draws on.
