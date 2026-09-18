# Project Coordinator Profile — Executive

## Status

<<<<<<< HEAD
The opt-in profile is implemented as a feature-branch candidate. ADR-055 records the bounded supersession of ADR-049 for ordinary Project Coordinator profiles. Exact-head qualification requires green local and hosted gates plus fresh review on the published head.
||||||| parent of ce1663549 (docs: keep coordinator contract in normative specs)
The opt-in profile is implemented as a qualified candidate on its feature branch. Focused persistence, API, runtime, compaction, and UI suites pass. The branch-wide gate is blocked only by ADR sequence prerequisites: ADR-054 and ADR-055 are reserved in other delivery branches, while this decision is allocated ADR-056.
=======
The opt-in profile is implemented as a candidate on its feature branch. Focused persistence, API, runtime, compaction, and UI suites pass. Exact-head qualification is established only when every required hosted check is green.
>>>>>>> ce1663549 (docs: keep coordinator contract in normative specs)

## Scope

The profile adds one ProductConversation-owned plain-text charter, generic coordination prompt guidance, coordination-oriented continuation compaction, and a bounded human editor. It does not reuse or modify the singleton Global Coordinator role and does not grant tools, authority, scheduling, subscriptions, or background execution.

## Trust boundary

Persisted edits are available through the human-facing HTTP settings action and are absent from Phoenix's LLM tools, chat ingestion, tool-result processing, and coordinator capabilities. Same-user HTTP authentication is not actor attestation; arbitrary misuse of an authenticated client is outside the claimed boundary.

## Verification coverage

| Requirement | Intended evidence |
|---|---|
| REQ-PCO-001, REQ-PCO-002, REQ-PCO-007 | `project_coordinator_profile` database tests cover default row absence, exact text, UTF-8/NUL bounds, restart, stable identity lookup, concurrent/stale CAS, clear, ordinary-only enforcement, and the kind fence. |
| REQ-PCO-003 | The router test covers save/conflict/clear/bounds; `ProductConversationPage.test.tsx` covers enable, disable, cancel, retained failure state, and Global Coordinator ineligibility. |
| REQ-PCO-004 | `project_coordinator_tests` proves generic wording and ordinary default behavior; the recorded-request runtime test proves the opt-in guidance and current charter blocks. |
| REQ-PCO-005 | `project_coordinator_policy_is_role_appropriate_and_excludes_charter` and `project_coordinator_compaction_preserves_rejected_tool_intent` cover role selection, delivery state, excluded charter, and pending tool intent. |
| REQ-PCO-006 | Schema/API eligibility and prompt dispatch preserve ordinary runtime role and reject the Global Coordinator. Mutation inventory confirms no profile or charter registration in Phoenix LLM tool registries. |

## Qualification

<<<<<<< HEAD
ADR-055 and migration 101 were allocated against the rebased mainline. ADR-055 supersedes ADR-049’s exclusions of an ordinary-conversation purpose setting, Project Coordinator classification, durable profile persistence, and profile-specific ordinary compaction while preserving ADR-049’s protected-handoff and durable-operation decisions. Exact-head qualification requires green local and hosted gates plus fresh review.
||||||| parent of ce1663549 (docs: keep coordinator contract in normative specs)
ADR-056 and migration 102 were allocated after inventorying open delivery branches. The task records mandatory re-inventory and renumbering before rebase/publication if those claims change. The local broad gate's code, UI, formatting, task, generated-artifact, and end-to-end lanes pass after focused fixes; its spec-shape lane cannot pass until the separately reserved ADR-054 and ADR-055 files reach the branch base. This is an integration-order dependency, not a waiver of the shape gate.
=======
Migration 102 was allocated independently after inventorying open delivery branches; the task records mandatory re-inventory and renumbering before rebase if that claim changes. No new project ADR is required: the normative requirements own this bounded profile contract, and existing ADRs own continuation durability, local SQLite authority, provider prompt projections, and compaction behavior. The full local and hosted gates must pass without a spec-shape exception.
>>>>>>> ce1663549 (docs: keep coordinator contract in normative specs)
