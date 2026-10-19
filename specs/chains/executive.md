# Phoenix Chains — Executive Summary

## Requirements Summary

The chains spec describes one ProductConversation with continuation-linked transcript history and read-only recall. The naming source in PR #951 makes the root title authoritative and retires duplicate naming mutations. Legacy root-keyed Q&A persistence and archive/delete compatibility surfaces remain; this is not a claim that every chain-specific lifecycle divergence is removed.

## Current Source Reality

`/chains/:rootConvId` redirects to the ordinary conversation surface. The unrouted ChainPage management implementation is retired; its live `ChainQaColumn` is retained within ProductConversationPage. Sidebar/list naming uses ProductConversation identity and authoritative aggregate responses. The backend retains root-keyed chain snapshot, Q&A and archive/delete compatibility routes, but not chain-name or transcript-name mutation routes. Historical read aliases, immutable UUIDs and exact evidence are preserved.

Code anchors are `api/chains.rs`, `api/handlers.rs`, `App.tsx`, `ProductConversationPage`, `ChainQaColumn` and `chainReducer`.

## Technical Summary

Migration129 folds effective names into canonical root `title` and removes persisted `chain_name`; `chain_qa` remains persisted. Manual and generated naming share ProductConversation validation, immutable ownership, admission and transactional projection allocation. Generated work is fenced by an opaque naming revision, including A→B→A changes. ADR-131 records the authority decision; ADR-132 records audited mutation retirement.

Lineage Q&A retains its chain-scoped stream and reducer. The answer phase emits `chain_qa_answer_reset` between rejected tool-calling attempts; failure preserves only the final attempt's partial text. ProductConversationPage hosts the live Q&A component; direct component and routed Recall tests retain streaming, read-only and error/reask coverage.

## Source Coverage and Remaining Gaps

| Requirement | Source coverage / remaining gap |
|---|---|
| REQ-CHN-001 | Retrieval-backed read-only Q&A and persisted answers retained. |
| REQ-CHN-002 | ProductConversation identity drives ordinary navigation; root-keyed compatibility representation remains in chain retrieval/Q&A. |
| REQ-CHN-003 | Ordinary ProductConversation surface hosts lineage selection; legacy chain URL redirects. |
| REQ-CHN-004 | Live `ChainQaColumn` is hosted on ProductConversationPage; separate chain-scoped SSE remains. |
| REQ-CHN-005 | `chain_qa` persistence remains root-row keyed rather than aggregate-owned. |
| REQ-CHN-006 | Stateless independent per-question invocation retained. |
| REQ-CHN-007 | Root title is sole authority; migration129 and canonical manual/generated operations replace duplicate mutation routes. |
| REQ-CHN-008 | Ordinary live conversation receives latest WorkScope identity; no separate ChainPage work-scope dock remains. |
| REQ-CHN-009 | Retrieval loop remains read-only and lineage-scoped; root-keyed internals remain. |
| REQ-CHN-010 | Duplicate naming mutations and unrouted chain management UI retired; archive/delete compatibility endpoints remain a lifecycle gap. |

## Qualification Boundary

Naming cleanup source `c1ed17600` passed exact normal CI39187737093 and macOS39187737161 with a clean exact-head review. Installer/producer actual main is integrated in this branch, but the integration successor requires its own hosted checks and review, tracked on PR #951. No production migration, deployment or browser/device acceptance is implied. Status updates belong on the PR rather than progress-only source commits.
