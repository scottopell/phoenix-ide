---
created: 2026-04-29
priority: p1
status: ready
artifact: src/api.rs
---

Add API endpoints in src/api/. GET /api/chains/:rootId returns ChainView { members: Vec<ConversationSummary>, qa_history: Vec<ChainQaRow>, chain_name: Option<String> }. POST /api/chains/:rootId/qa with { question } returns { chain_qa_id }. PATCH /api/chains/:rootId/name with { name } updates conversations.chain_name on the root (validates root_conv_id is actually a chain root). GET /api/chains/:rootId/stream is the SSE subscription endpoint for chain Q&A token streams. Wire all four to the runtime + Q&A backend + broadcaster. Add API types with ts_rs derive for codegen into ui/src/generated/. Tests: handler behavior including 404 for non-roots, validation, etc.
