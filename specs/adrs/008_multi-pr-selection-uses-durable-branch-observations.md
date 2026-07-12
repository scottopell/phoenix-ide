# ADR-008: Multi-PR selection uses durable settled-branch observations plus explicit active-PR targeting

- **Status:** Accepted
- **Date:** 2025-08-08
- **Affects:** REQ-PRA-000, REQ-PRA-000a, REQ-PRA-000b, REQ-PRA-000c, REQ-BASH-010a, REQ-BASH-010b, REQ-BASH-010c

## Context

Phoenix supports Work and Branch conversations that own one worktree lifecycle while the user and
agent may create more than one deliverable pull request from that work. Stacked and sibling PRs
make the old hidden singular-primary model insufficient: Phoenix needs durable evidence of which
branches mattered to the task, a way to retain multiple PR associations without mirroring all of
GitHub, and one explicit target for PR-specific surfaces so the StateBar, status reads, feedback
freshness, and Address-CI action do not silently point at different PRs.

Phoenix also needs an observation source that is cheap, local, and structurally honest. Parsing
shell commands, shell traces, or `gh` output guesses at intent rather than observing the resulting
repository state. Git hooks and filesystem watchers widen Phoenix's ownership boundary into the
user's repository and local environment. Exposing raw WorkScope or branch-registration APIs to
agents would make correctness depend on the agent remembering to annotate state Phoenix can derive
for itself.

The first implementable source Phoenix already owns is the bash handle lifecycle edge. When a bash
wrapper process reaches terminal state, Phoenix can observe the authoritative settled Git state of
the worktree, record one branch observation, and trigger bounded PR refresh work keyed by
WorkScope.

## Options considered

1. **Continue with one hidden primary PR per WorkScope** — keep the singular-primary ranking as the
   only authority for status and actions. This preserves existing behavior and avoids new domain
   types. Its cost is correctness: multiple deliverable PRs collapse into one hidden choice, and
   PR-specific surfaces can silently retarget as branch observations change.
2. **Use durable settled-branch observations plus explicit active-PR targeting** — record observed
   task-branch heads at supported reconciliation boundaries, retain plural PR association history,
   and distinguish inferred active selection from user-pinned selection. This keeps Phoenix's
   observation model cheap and local while making PR-specific targeting explicit. Its cost is added
   domain and migration work across API, UI, and persistence.
3. **Infer branch and PR changes from command parsing, hooks, or explicit agent registration** —
   parse bash commands or `gh` output, install Git hooks or background observers, or expose tools
   that let agents register branches/PRs directly. This can appear more immediate than waiting for
   terminal lifecycle edges. Its cost is a weaker correctness model and broader ownership: parsed
   intent can diverge from final Git state, hooks/watchers intrude into user environments, and
   explicit registration makes omissions indistinguishable from bugs.

## Decision

Phoenix uses **durable settled-branch observations plus explicit active-PR targeting**.

The authoritative local observation source is the bash terminal lifecycle edge. At that boundary,
Phoenix observes final settled Git state, records a WorkScope-keyed branch observation when the
state qualifies as a task-branch deliverable candidate, and uses the durable candidate set to drive
bounded PR discovery. Phoenix retains discovered PR associations by full repository-plus-PR-number
identity even after the worktree later checks out another branch or the local branch disappears.

PR-specific surfaces do not target a hidden singular primary PR. They target one explicit **active
PR** whose provenance is structural: either **pinned** by the user or **inferred** from durable
branch and PR facts. Inference favors the latest settled observed actionable branch match, then the
only actionable associated PR, then a still-valid prior inferred selection. If more than one
plausible actionable PR remains, Phoenix leaves selection ambiguous rather than silently choosing by
recency.

A singular ranked primary-PR view may remain during migration, but only as a compatibility
projection for surfaces that have not yet been migrated. It is not the hidden authority for active
multi-PR actions.

Phoenix does **not** parse commands as the correctness mechanism, install Git hooks, expose raw
WorkScope construction or PR-registration responsibility to agents, or promise observation of
intermediate branch transitions inside one process. Recovery for transient unobserved branches comes
from later authoritative observations at supported reconciliation boundaries.

## Consequences

- **Positive:** PR-specific targeting becomes explicit and testable. Stacked and sibling PR history
  can be retained without inventing multiple task owners. Phoenix observes authoritative Git state
  rather than parsed intent, and the bash terminal edge provides a cheap first observation source.
- **Negative:** The model introduces new persisted branch-observation and active-selection concepts,
  plus migration work across APIs, UI surfaces, and status derivation. Some PR-specific surfaces
  must remain compatibility-backed until the migration completes.
- **Neutral:** Phoenix remains intentionally non-omniscient. Branch transitions that occur only
  transiently inside one still-running process, or outside supported observation sources, are not
  captured until a later supported reconciliation boundary observes them.

## References

- ADR-000
- `specs/pr-association/requirements.md`
- `specs/pr-association/pr-association.allium`
- `specs/bash/requirements.md`
- `specs/bash/bash.allium`
- `specs/work-actions-bar/requirements.md`
- `specs/work-lifecycle/requirements.md`
