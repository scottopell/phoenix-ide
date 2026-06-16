Two related durability holes in the tool-round persistence path; both can lose work the user already saw.

## F1 — in-flight tool round dropped on restart (HIGH)
The assistant message + completed tool results live only in the `ToolExecuting` state JSON until the end-of-round `PersistCheckpoint`; they are eagerly broadcast over SSE (effect.rs BroadcastAssistantMessage) but not written to `messages`. On restart, `reset_all_to_idle` (phoenix-db lib.rs ~2590) overwrites that state with {"type":"idle"} WITHOUT materializing its contents into `messages`. A deploy/SIGHUP/crash mid-round silently drops the assistant turn the user already read plus the real outputs of already-completed tools; the conversation rewinds to the prior user message with no marker.
Fix: at startup, before reset, materialize `tool_executing` rows (`assistant_message` + `completed_results`) into `messages`, synthesizing an error result only for the genuinely-incomplete current tool, then reset.

## F2 — persist_checkpoint is N separate INSERTs, no transaction (HIGH)
runtime/executor.rs persist_checkpoint (~2403) is documented atomic but inserts the assistant message then each tool result separately. Partial failure (SQLITE_BUSY past timeout, crash) leaves an unpaired tool_use that 400s every subsequent LLM call until restart, and restart repair replaces the real tool output with "[interrupted by server restart]" — permanent loss. Template already exists: Database::persist_fork_proposal_with_tool_round (phoenix-db lib.rs ~2177) wraps the identical shape in one transaction.
Fix: add transactional persist_tool_round(agent_msg, tool_msgs) and route persist_checkpoint through it.

Found in spiritual-core audit 2026-06-10. Verified anchors against source.
