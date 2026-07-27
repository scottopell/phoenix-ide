# Phoenix Chains — Executive Summary

## Requirements Summary

The chains spec describes Phoenix's existing continuation-chain product surface: derived chain identity over `continued_in_conv_id`, a dedicated chain page, persisted chain Q&A, editable chain naming, and chain-scoped retrieval for read-only recall.

## Current Reality

Chains are still fully shipped current behavior. The UI still exposes a dedicated chain route (`/chains/:rootConvId`), chain grouping in the sidebar, chain archive/delete endpoints, and chain Q&A streaming. This remains an explicit implementation divergence from the newer unified-conversation normative direction: continuation is still surfaced as a first-class chain product concept rather than only as one conversation with multiple transcript segments.

Code anchors for that current reality include `crates/phoenix-ide/src/api/chains.rs`, route registration in `crates/phoenix-ide/src/api/handlers.rs`, lazy `ChainPage` routing in `ui/src/App.tsx`, and sidebar navigation to `/chains/...` in `ui/src/components/ConversationList.tsx`.

## Technical Summary

Chains remain a derived layer over `conversations.continued_in_conv_id` plus persisted `chain_name` and `chain_qa` data. The chain page still resolves work identity from the active member, and chain-scoped SSE/Q&A remain separate from ordinary conversation SSE.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-CHN-001:** Recall Past Work Without Re-Explaining Context | ✅ Complete (legacy current reality) | Chain-scoped read-only Q&A remains shipped |
| **REQ-CHN-002:** Continuation Chains Surface as First-Class Entities | ✅ Complete (legacy current reality) | Derived chain identity and sidebar grouping remain shipped |
| **REQ-CHN-003:** Chain Page as a Navigable Place | ✅ Complete (legacy current reality) | Dedicated chain route/page still exists |
| **REQ-CHN-004:** Ask the Chain, Get a Streamed Answer | ✅ Complete (legacy current reality) | Chain Q&A SSE remains shipped |
| **REQ-CHN-005:** Q&A History Persists Per Chain | ✅ Complete (legacy current reality) | `chain_qa` persistence remains shipped |
| **REQ-CHN-006:** Consistent Quality As Q&A Accumulates | ✅ Complete (legacy current reality) | Stateless per-question invocation remains shipped |
| **REQ-CHN-007:** Chain Has a User-Editable Name | ✅ Complete (legacy current reality) | Editable/regenerated `chain_name` remains shipped |
| **REQ-CHN-008:** Chain Page Surfaces Work Identity Alongside Runtime Resources | ✅ Complete (legacy current reality) | Chain dock/work-identity surfaces remain shipped |
| **REQ-CHN-009:** Chain Q&A Is a Read-Only Agentic Loop | ✅ Complete (legacy current reality) | Retrieval-backed loop remains shipped |
| **REQ-CHN-010:** Regenerate Chain Name From Member Content | ✅ Complete (legacy current reality) | Regenerate endpoint/button remains shipped |

## Reconciliation Note

This executive intentionally reports chains as current implementation, not as the desired long-term product model. The unified lifecycle work in sibling specs aims to remove the dedicated chain page and treat continuation as one conversation aggregate, but that cutover has not landed in code.
