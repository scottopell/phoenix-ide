---
created: 2026-02-28
number: 575
priority: p1
status: done
slug: checkpoint-tool-round-persistence
title: "Atomic persistence via CheckpointData::ToolRound — hold assistant message until all tools complete"
---

# Atomic Tool Round Persistence

## Context

Read first:
- `specs/bedrock/design.md` — "Persistence Model (REQ-BED-007, FM-2 Prevention)" section
- `specs/bedrock/design.md` — Appendix A (FM-2, FM-4)
- `specs/bedrock/requirements.md` — REQ-BED-007

FM-2: The executor persisted the agent message (with `tool_use` blocks) immediately on
LLM response, then launched tools. On crash, storage had `tool_use` without matching
`tool_result`. LLM API rejected the malformed history.

FM-4: `completed_results` and `persisted_tool_ids` were parallel representations of
"what has been persisted." They could diverge.

## What to Do

1. **Create `CheckpointData` enum** (or equivalent) with a `ToolRound` variant that
   requires both `AssistantMessage` and `Vec<ToolResult>`. The constructor must enforce
   matching counts:

   ```rust
   impl CheckpointData {
       pub fn tool_round(msg: AssistantMessage, results: Vec<ToolResult>) -> Result<Self, PersistError> {
           if msg.tool_uses().len() != results.len() {
               return Err(PersistError::ResultCountMismatch { ... });
           }
           Ok(Self::ToolRound { assistant_message: msg, tool_results: results })
       }
   }
   ```

2. **Modify `ToolExecuting` state** to hold `assistant_message: AssistantMessage` —
   the message is NOT persisted on LLM response. It lives in state until all tools
   complete.

3. **Remove `persisted_tool_ids`** (or equivalent tracking set) from `ToolExecuting`.
   `completed_results` is the single source of truth. There is no parallel "what has
   been persisted" representation.

4. **Change the persistence call** when the last tool completes: emit a
   `PersistCheckpoint(CheckpointData::tool_round(assistant_message, all_results))`
   effect instead of persisting incrementally.

5. **Update the persistence layer** (`db::persist_checkpoint` or equivalent) to accept
   `CheckpointData` and write both assistant message and tool results atomically.

6. **Verify crash recovery**: on restart, conversations should resume from idle with
   consistent message history. No orphaned `tool_use` without `tool_result`.

## Acceptance Criteria

- `CheckpointData::tool_round()` constructor rejects mismatched counts
- No `persisted_tool_ids` or equivalent tracking set in tool executing state
- Assistant message is not written to DB until all tools complete
- `./dev.py check` passes
- Property tests verify: every `PersistCheckpoint::ToolRound` in effects has matching
  tool_use/tool_result counts

## Dependencies

- None (can be done independently of task 574)

## Clarification: AssistantMessage Must Be a Concrete Type

**Do NOT thread `Vec<ContentBlock>` + `UsageData` + `display_data` + `message_id` as
separate fields.** Introduce `AssistantMessage` as a proper struct:

```rust
pub struct AssistantMessage {
    pub message_id: String,
    pub content: Vec<ContentBlock>,
    pub usage: Option<UsageData>,
    pub display_data: Option<serde_json::Value>,
}

impl AssistantMessage {
    /// Returns the tool_use blocks from content. Used by CheckpointData::tool_round()
    /// to enforce the matching-count invariant.
    pub fn tool_uses(&self) -> Vec<&ContentBlock> {
        self.content.iter().filter(|b| matches!(b, ContentBlock::ToolUse { .. })).collect()
    }
}
```

This is load-bearing for the design:
- `ToolExecuting` holds ONE field (`assistant_message: AssistantMessage`), not four
  loose fields that can be partially threaded or forgotten
- `CheckpointData::tool_round()` takes `AssistantMessage` — you can't call it with
  content blocks but no usage data, because they're bundled
- Future tasks (577: typed effects) need `AssistantMessage` as a concrete type in
  `LlmOutcome::Response(AssistantMessage, TokenUsage)`

The whole point of this refactor is correct-by-construction. Four loose fields that
happen to travel together is exactly the kind of implicit coupling that creates bugs.
Bundle them into the struct now.

## Files Likely Involved

- `src/state_machine/state.rs` — ToolExecuting state definition, AssistantMessage struct
- `src/state_machine/transition.rs` — LlmResponse and ToolComplete transitions
- `src/state_machine/effect.rs` — Effect enum, CheckpointData type
- `src/db/` — persistence layer
- `src/runtime/executor.rs` — effect execution
