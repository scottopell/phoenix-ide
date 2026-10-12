# ADR-095: WorkScope CPU history with demand-driven memory

- **Status:** Accepted
- **Date:** 2026-10-11
- **Affects:** REQ-WSLOAD-001 through REQ-WSLOAD-005, REQ-DEPLOY-007a

## Context

The resource monitor supplied request-coalesced instantaneous observations for deployment, WorkScope inventory, and process inspection. The user approved a first-party per-WorkScope/across-scope API, a Global-only tool, and small inline indicators. They explicitly requested always-on recent consumed-CPU tracking, accepted existing owned bash process groups and exclusion of detached descendants, and chose in-memory history and fresh demand memory. Disk and run-queue additions were rejected.

## Options considered

1. Retain demand-only collection: cheap idle behavior, but no history before the first query.
2. Reuse full demand observations periodically: supplies history but unnecessarily reads memory, terminal/API processes, and host metrics.
3. Add cumulative CPU history within one WorkScope summary service and reuse the existing monitor for demand memory.

## Decision

Choose option 3. One runtime-owned summary service samples cumulative consumed CPU approximately every five seconds while tracked bash groups are live. It retains a bounded fifteen-minute ring, uses native process incarnation identity, and reports measured coverage rather than filling missing intervals. Restart resets history. Memory remains behind the shared demand-observation lease; API and tool consume the same summary projection.

Ownership determines accounting, including Coordinator-targeted jobs. Conflicting ownership or incomplete observations become unavailable, not arbitrary partial totals. Ordinary API callers are limited to their attached scope; the current Global identity may query across scopes. Only Global receives the tool, with execution-time identity admission.

The existing WorkScope section carries inline indicators. Live readiness and matching scope inventory admit UI polling; replay and archived surfaces remain inert. No automatic context injection or resource control is introduced.

## Consequences

- The demand-only prohibition on all recurring process samples in REQ-DEPLOY-007a is narrowed to memory/host collection; CPU history is explicitly governed by REQ-WSLOAD-002.
- Membership churn and jobs shorter than the sampling cadence can reduce measured coverage. The API exposes this limitation rather than estimating missing work.
- Native reads occur only for tracked live groups in the background; no persistence, new monitoring framework, disk walks, or recovery mechanism is needed.
- Inline presentation and agent consumers share measurement semantics without adding a dedicated resource page.
