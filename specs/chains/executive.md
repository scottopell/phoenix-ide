# Phoenix Chains — Executive Summary

## Requirements Summary

The chains spec describes the normative unified ProductConversation surface for continuation-linked transcript history and read-only recall. The shipped implementation still uses a derived root-keyed chain identity, dedicated chain page, persisted chain Q&A, and editable chain naming as compatibility behavior.

## Current Reality

Chains are still fully shipped current behavior. The UI still exposes a dedicated chain route (`/chains/:rootConvId`), chain grouping in the sidebar, chain archive/delete endpoints, and chain Q&A streaming. This remains an explicit implementation divergence from the newer unified-conversation normative direction: continuation is still surfaced as a first-class chain product concept rather than only as one conversation with multiple transcript segments.

Code anchors for that current reality include `crates/phoenix-ide/src/api/chains.rs`, route registration in `crates/phoenix-ide/src/api/handlers.rs`, lazy `ChainPage` routing in `ui/src/App.tsx`, and sidebar navigation to `/chains/...` in `ui/src/components/ConversationList.tsx`.

## Technical Summary

Chains remain a derived layer over `conversations.continued_in_conv_id` plus persisted `chain_name` and `chain_qa` data. The chain page still resolves work identity from the active member, and chain-scoped SSE/Q&A remain separate from ordinary conversation SSE.

## Status Summary

| Requirement | Status | Notes |
|---|---|---|
| **REQ-CHN-001:** Recall Past Work Without Re-Explaining Context | ✅ Complete (legacy current reality) | Chain-scoped read-only Q&A remains shipped |
| **REQ-CHN-002:** Continuation Lineage Is Navigation Topology, Not a Separate Product Entity | Not implemented | Shipped derived root-keyed chain identity and sidebar grouping remain; first-class ProductConversation aggregate identity is not yet the navigation authority. |
| **REQ-CHN-003:** The Normal Conversation Surface Hosts Lineage History | Not implemented | Shipped behavior still uses the dedicated chain route/page. |
| **REQ-CHN-004:** Lineage Q&A Streams on the Normal Conversation Surface | 🟡 Partial | Streaming lineage Q&A is shipped on the legacy chain page, not the normal ProductConversation surface. |
| **REQ-CHN-005:** Q&A History Persists With the Product Conversation | 🟡 Partial | `chain_qa` persistence remains root-row/chain keyed rather than aggregate-owned. |
| **REQ-CHN-006:** Independent Q&A Quality As History Accumulates | ✅ Complete (legacy current reality) | Stateless per-question invocation remains shipped. |
| **REQ-CHN-007:** Conversation Title Belongs to the ProductConversation Aggregate | Not implemented | Editable/regenerated `chain_name` remains a separate shipped authority. |
| **REQ-CHN-008:** The Normal Conversation Surface Shows Work Identity for the Live Attached Scope | 🟡 Partial | Work identity is shipped on the legacy chain page rather than the normal ProductConversation surface. |
| **REQ-CHN-009:** Lineage Q&A Is a Read-Only Agentic Loop Scoped to One Product Conversation | 🟡 Partial | The retrieval-backed loop is shipped but remains bound to root-keyed chain identity. |
| **REQ-CHN-010:** No Separate Chain-Specific Lifecycle or Management Actions | Not implemented | Dedicated chain naming, archive/delete, and management endpoints remain shipped. |

## Reconciliation Note

This executive intentionally reports chains as current implementation, not as the desired long-term product model. The unified lifecycle work in sibling specs aims to remove the dedicated chain page and treat continuation as one conversation aggregate, but that cutover has not landed in code.
