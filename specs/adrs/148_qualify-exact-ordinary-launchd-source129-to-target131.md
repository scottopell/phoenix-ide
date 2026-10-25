# ADR-148: Qualify exact ordinary launchd source129 to target131

- **Status:** Accepted
- **Date:** 2026-10-25
- **Affects:** REQ-COMP-019, REQ-LDD-028; REQ-COMP-013, REQ-LDD-023

## Context

The context-capacity candidate adds migration 130's normalized immutable handoff
review candidates and migration 131's relational capacity plans, usage identity,
and observation coverage. Production's reported source ledger is 129. The ordinary
helper qualifies only 123-to-128; the protected 128-to-129 and private 129 refresh
routes retain immutable historical candidate registry pins. Replacing those pins
would silently invalidate the proof contracts they protect.

## Options considered

1. Reuse a protected route by changing its historical registry pin. This changes
   an existing qualification and falsely borrows snapshot/rehearsal authority.
2. Introduce a generic forward migration/recovery framework. This expands the
   compatibility promise beyond this release's needs.
3. Add one exact ordinary129-to-131 qualification to the existing candidate-start
   forward-only lifecycle, retaining all historical qualifications independently.

## Decision

Choose option 3. Qualify only complete source129 to the complete registry whose
SHA-256 is `c3911ab5dcf05dc4c19472486ae463179948d692370976ed3c81ee6948445744`.
Its final entries are 129 `single_conversation_name_and_route_projection`, 130
`continuation_handoff_candidates`, 131 `request_capacity_diagnostics`.
The staged helper owns pre-handoff admission; full registry bytes, digest, exact
candidate and configuration remain immutable retained transaction evidence.

Use the existing ordinary start-attempt checkpoint. Before candidate startup,
unchanged-predecessor artifact rollback remains available. After startup is
attempted, retain candidate/evidence/claim and never start the predecessor or
replay a snapshot. Migration 130 can commit before 131 fails. Explicit
`resume-forward` re-verifies retained evidence and permits only exact complete
source129, partial130 or target131 ledgers, starting only the retained candidate.
A source already at target131 uses ordinary no-pending behavior.

Do not require a standalone ordinary backup or represent protected snapshot or
rehearsal proof as present. An operational one-off backup is separate from this
qualification. Preserve exact 123-to-128 ordinary and every protected historical
pin and lifecycle. A missing registry or contradictory ownership remains a
failure requiring diagnosis, never permission to clear a claim.

Historical tests use committed compressed byte-exact schema129 registry evidence
from `9ebd9d8d42876c3356557f4ab461b3b8ffabd743` (decompressed SHA-256
`c54f9884b1b86aea19ca214cf059724f456c899a219006b59f1c6a16cca6caa5`).
Shallow CI clones need no old git objects. Synthetic schema128 fixtures derive
from those bytes and scope their own unit pins; production constants stay fixed.

## Consequences

- The intended additive release has a bounded native ordinary activation route.
- A future registry needs independent qualification; post-start failure may need
  exact-candidate forward recovery instead of automatic availability restoration.
- Disposable SQLite/launchd-double tests prove admission, evidence and lifecycle
  branches; they do not prove actual migration SQL, real launchd activation,
  hosted CI acceptance, production deployment or behavioral acceptance.

## References

- [Compatibility requirements](../compatibility/requirements.md), REQ-COMP-019.
- [Launchd requirements](../launchd-deployment/requirements.md), REQ-LDD-028.
- [ADR-105](105_qualify-migration-128-for-bounded-current-schema-activation.md).
