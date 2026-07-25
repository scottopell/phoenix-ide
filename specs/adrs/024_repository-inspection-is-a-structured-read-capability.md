# ADR-024: Repository inspection is a structured read capability

- **Status:** Accepted
- **Date:** 2026-07-25
- **Affects:** REQ-RI-001–008, REQ-GR-007

## Context

The Coordinator can correlate relational evidence but cannot independently inspect the source behind active branches. Granting the existing Work toolset would also grant mutation authority. Reusing arbitrary read-looking shell commands would leave mutation prevention dependent on prompts and command classification, while resolving a repository from caller cwd would make filesystem location an accidental authority token.

## Options considered

### Grant Coordinator the Work toolset

Rejected because inspection does not require filesystem or repository mutation and the extra authority cannot be structurally constrained to the user need.

### Permit a read-only Bash profile

Rejected because shell composition, executable selection, aliases, hooks, pagers, redirects, and subprocess behavior make the non-mutation contract dependent on an open command language.

### Add structured repository operations resolved from persisted WorkScope identity

Selected because the operation vocabulary, target authority, execution bounds, and evidence shape can all be enforced below the model-facing description.

## Decision

Phoenix provides repository inspection as a distinct stateless capability. The caller selects a persisted WorkScope identity; the server resolves and authorizes an active repository-backed target. The model chooses only a typed operation and validated fields. An internal runner constructs allowlisted Git argv with noninteractive, no-external-program configuration and fixed resource bounds.

Coordinator may inspect an explicitly selected active repository scope. Restricted conversations may inspect only their own scope. The capability does not grant Work authority or expose existing Work tools.

Network access is not part of repository inspection. Pull-request metadata, checks, and remote ref fetching require a separate explicit capability decision.

## Consequences

- Branch compatibility can be grounded in exact local commits, diffs, names, and source locations.
- Repository location is evidence returned by authorization, not a model-supplied authority handle.
- The operation vocabulary is intentionally smaller than Git and can grow only through explicit typed additions and tests.
- Local refs must already exist; network freshness is outside this capability.
