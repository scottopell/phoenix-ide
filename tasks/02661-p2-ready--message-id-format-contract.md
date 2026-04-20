---
created: 2026-04-20
priority: p2
status: ready
artifact: src/runtime/executor.rs
---

# Thread message ID through state instead of format! convention

## Summary

PersistCheckpoint and PersistSubAgentResults share a message ID via
`format!("{tool_id}-result")`. This implicit contract breaks silently if
either side changes the format. The SpawnAgentsComplete event should carry
a message_id that gets stored in AwaitingSubAgents state.

## Done When

PersistSubAgentResults reads the message_id from state, not from convention.
No `format!("{tool_id}-result")` pattern matching remains.
