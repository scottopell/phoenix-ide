# Project Coordinator Profile — Executive

## Status

The opt-in profile is implemented as a feature-branch candidate. ADR-075 records the bounded supersession of ADR-049 for ordinary Project Coordinator profiles. Exact-head qualification requires green local and hosted gates plus fresh review on the published head.

## Scope

The profile adds one ProductConversation-owned plain-text charter, generic coordination prompt guidance, coordination-oriented continuation compaction, and a bounded human editor. It does not reuse or modify the singleton Global Coordinator role and does not grant tools, authority, scheduling, subscriptions, or background execution.

## Trust boundary

Persisted edits are available through the human-facing HTTP settings action and are absent from Phoenix's LLM tools, chat ingestion, tool-result processing, and coordinator capabilities. Same-user HTTP authentication is not actor attestation; arbitrary misuse of an authenticated client is outside the claimed boundary.

## Verification coverage

| Requirement | Intended evidence |
|---|---|
| REQ-PCO-001, REQ-PCO-002, REQ-PCO-007 | `project_coordinator_profile` database tests cover default row absence, exact text, UTF-8/NUL bounds, restart, stable identity lookup, concurrent/stale CAS, clear, direct-insert revision fencing, ordinary-only enforcement, sub-agent rejection, and the kind fence. |
| REQ-PCO-003 | `ProductConversationPage.test.tsx` covers enable, disable, cancel, conflict handling, bounds feedback, retained failure state, automatic continuation visibility, aggregate invalidation, reconciliation refresh, draft preservation, and Global Coordinator ineligibility. Database tests cover save, stale CAS, clear, bounds, and ordinary-only persistence enforcement. |
| REQ-PCO-004 | `project_coordinator_tests` proves generic wording and ordinary default behavior. `ordinary_project_coordinator_profile_reaches_provider_request` records the opt-in guidance and charter blocks in the frozen provider request; runtime profile lookup failure stops before provider dispatch. |
| REQ-PCO-005 | `project_coordinator_policy_is_role_appropriate_and_excludes_charter`, `project_coordinator_compaction_preserves_rejected_tool_intent`, and the compaction lookup-failure regression cover role selection, delivery state, excluded charter, pending tool intent, and fail-closed profile reads. |
| REQ-PCO-006 | Schema/API editability and prompt dispatch preserve ordinary runtime role and reject the Global Coordinator and sub-agent owned conversations. Mutation inventory confirms no profile or charter registration in Phoenix LLM tool registries. |

## Qualification

ADR-075 and migrations 113–119 are allocated after the provenance and startup-fingerprint mainline history. ADR-075 supersedes ADR-049’s exclusions of an ordinary-conversation purpose setting, Project Coordinator classification, durable profile persistence, and profile-specific ordinary compaction while preserving ADR-049’s protected-handoff and durable-operation decisions. Exact-head qualification requires green local and hosted gates plus fresh review.
