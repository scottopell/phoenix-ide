---
created: 2026-04-29
priority: p1
status: ready
artifact: src/runtime.rs
---

Implement chain Q&A backend module. Public API: submit_question(root_id, question) returns (chain_qa_id, broadcaster_handle); load_history(root_id) returns Vec<ChainQaRow>. Internal: context bundling collects continuation summary (MessageType::Continuation, tail of each non-leaf member) plus leaf transcript (if message_count <= 20 and approx tokens <= 4000) or in-process leaf summary (cheap mid-tier model call, discarded after). Persist row at submit with status=in_flight, snapshot_member_count and snapshot_total_messages computed via the chain walk + summing each member Conversations message_count field. Update row to completed/failed on stream resolve. Add startup_sweep() called from main.rs that transitions all in_flight rows to abandoned. Use existing LLM provider abstraction with mid-tier model identifier. Tests: bundling correctness, snapshot integers correct, status transitions.
