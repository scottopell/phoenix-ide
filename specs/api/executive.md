# HTTP API - Executive Summary

## Requirements Summary

The HTTP API exposes conversation listing, creation, chat submission, SSE streaming, directory/model helpers, and conversation lifecycle endpoints used by the shipped UI.

## Current Reality

The API still reflects the legacy lifecycle and chain model. Current shipped endpoints include archived-vs-active listing (`GET /api/conversations` and `/api/conversations/archived`), ordinary archive (`POST /api/conversations/:id/archive`), legacy terminal cleanup (`POST /api/conversations/:id/abandon-task`, `POST /api/conversations/:id/mark-merged`), chain routes under `/api/chains/:rootId/...`, and work-scope inventory endpoints under `/api/work-scope/:scope_key/...`. The future unified Open/History API shape has not landed yet, so this executive keeps those legacy endpoints visible as current behavior rather than implying the new normative lifecycle is already shipped.

## Technical Summary

REST/JSON plus SSE. The conversation SSE stream is specified to carry root-keyed `conversation_update`, typed `product_conversation_lifecycle`, continuation-boundary projection, and `work_scope_update` alongside persisted message, state-change, token, LLM lifecycle, and deletion events. Aggregate-bound live carriers are now normatively required to materialize as one closed typed envelope over the authoritative carrier set, sharing one RootStreamLedger ordering space across init, replay, and live delivery while carrying the root ProductConversation identity plus member identity where applicable. `work_scope_update` is aggregate-bound in that contract: it allocates a root-stream sequence, carries the root ProductConversation id plus inventory payload identity, omits any member-row owner, and appends to the same root replay/broadcast path. `init`, terminal `conversation_hard_deleted`, and the live-only steering queue projections remain outside that aggregate-bound replay envelope. The server continues to back both ordinary conversation routes and dedicated chain routes. Conversation creation remains the existing shell-first HTTP flow; the durable creation protocol is only partially cut over.

## Requirement Map

| Requirement | Surface | Current status note |
|-------------|---------|---------------------|
| **REQ-API-001:** Conversation Listing | GET /api/conversations and /archived | Implemented, still active/archived rather than Open/History |
| **REQ-API-002:** Conversation Creation | Slug: day-time-word-word format | Implemented; durable creation cutover incomplete |
| **REQ-API-003:** Message Retrieval | GET with `after_sequence` param | Implemented |
| **REQ-API-004:** User Actions | POST chat, cancel endpoints | Implemented |
| **REQ-API-031:** Explicit Public Literal Chat | `/chat` opt-in, closed policy, known receiver, policy-proven replay | Authored; offline qualification only, Rust/E2E execution pending |
| **REQ-API-005:** Real-time Streaming | SSE with init, token events (`request_id` for correlation), replay-backed reconnection | Implemented |
| **REQ-API-006:** Conversation Lifecycle | Archive, delete, rename | Partially implemented against current norms: archive/delete/rename ship, but unified Close/Open/History lifecycle does not |
| **REQ-API-007:** Slug Resolution | GET /api/conversation-by-slug/{slug} | Implemented |
| **REQ-API-008:** Directory Browser | validate-cwd and list-directory | Implemented |
| **REQ-API-009:** Model Information | GET /api/models with default | Implemented |
| **REQ-API-010:** Static Assets | UI assets served from the binary (embedded), with a filesystem fallback | Implemented |
| **REQ-API-013:** One-Shot Command Suggestion | POST /api/suggest | Implemented |

## Public literal chat

REQ-API-031 is authored in `ChatExpansionPolicy` and the shared `SendChatApplicationService`. Existing `/chat` keeps its expanding default and accepts explicit `literal_text`; no extra route or version flag is added. ADR-124 records the known-receiver verification and harmless retained-ID smoke recipe. Transcript-only replay remains default-policy-only unless durable receipts prove policy.

Router regressions cover literal idle/direct and running/steering admission, exact replay and changed-policy conflict, expanding default/unknown policy, pending manual-opening identity, closed/history refusal, authentication and attachment validation. The mock-provider E2E scenario `public_literal_chat` checks compiled-HEAD identity, a harmless retained-ID smoke, both routing states with causal queue observation, materialization and replay. Rust and real-binary E2E tests remain unexecuted locally under the no-build constraint. Eight offline guidance tests and 35 harness self-tests pass, including missing/malformed/mismatched receiver zero-POST checks and version rechecking before retry. No deployed behavior or live API acceptance is claimed.

## Verification Notes

Reconciled against `crates/phoenix-ide/src/api/handlers.rs`, `crates/phoenix-ide/src/api/lifecycle_handlers.rs`, `crates/phoenix-ide/src/api/chains.rs`, and `crates/phoenix-ide/src/api/sse.rs`.


## Continuous History API Qualification

REQ-API-030 is implemented in the dedicated Coordinator history read and shared aggregate paging projection. Its wire type intentionally has no ordinary lifecycle, Close, or work-action fields. Ordinary reads retain their existing domain gate. The web selects `boundary_representation=original_messages`: navigation metadata covers all persisted boundaries, message pages carry original continuation records, and history segments contain no parallel handoff summary. The default `handoff_metadata` representation remains available to existing consumers, including Swift, without changing their implementation. Pending successor openings do not advertise write readiness. The content-free boundary roster also identifies the exact represented successor opening, so live hydration and outgoing-member retention cannot restore that duplicate. Coordinator route membership returns an immutable root identity rather than a moving leaf identity. The pre-review source f8859137415063557bb21b4f236992f699b8ba4c passed genuine regeneration and focused execution of32 API tests and15 aggregate database tests (one existing ignored) using verified binaries under the host's compiler-free envelope. Those binary results do not qualify the later opening-authority repair. Actual exact eb6f33adb6 hosted run38328590822 subsequently completed successfully, including current-source Cargo test compilation/execution and all seven applicable jobs; unchanged Rust source may carry that execution proof, not the predecessor binary credit.

Genuine `./dev.py codegen` runs generated the Coordinator/boundary TypeScript types, including a guarded same-owner warm regeneration for the exact represented-opening identity. Independent cumulative static review passed its bounded source checks. Hosted CI passed all jobs for a47c02d1d, whose completed review found opening-authority, accessible-label and telemetry defects; those were repaired in eb6f33adb6. Hosted eb6 review5497631977 found a represented-opening bookmark regression and stale executive qualification wording. Those findings require successor UI/doc qualification and final-head review; exact-head live-server reader-anchor acceptance remains a separate resource-gated source journey. Physical user acceptance is a separate release gate. This source is not a production deployment record.
