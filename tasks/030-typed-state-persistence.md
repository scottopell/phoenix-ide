---
created: 2026-02-01
priority: p2
status: ready
---

# Typed State Persistence

## Summary

Replace the lossy `state` string + `state_data` JSON blob with a single fully-typed JSON column that serializes the entire `ConvState` enum.

## Context

Currently the DB stores conversation state as:
- `state`: A string like `"awaiting_sub_agents"`
- `state_data`: An optional JSON blob with some (but not all) state details

This causes problems:
1. **Data loss** - `AwaitingSubAgents.pending_ids` and `completed_results` are never persisted, so page reload shows 0/0 sub-agents
2. **Inconsistent** - Only some states (`LlmRequesting`, `ToolExecuting`, `Error`) extract data; others fall through to `None`
3. **Fragile** - `parse_state()` creates fake placeholder values (empty vecs, empty strings) that don't match runtime reality
4. **Duplicated logic** - State structure is defined in the enum AND manually extracted/reconstructed in two places

`ConvState` already derives `Serialize, Deserialize` with proper tagging:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConvState { ... }
```

This produces JSON like:
```json
{"type":"awaiting_sub_agents","pending_ids":["abc","def"],"completed_results":[]}
```

## Acceptance Criteria

- [ ] Single `state` column stores full `ConvState` as JSON
- [ ] Remove `state_data` column from schema
- [ ] Remove `parse_state()` function and placeholder logic
- [ ] Remove manual `state_data` extraction in `Effect::PersistState` handler
- [ ] `Conversation` struct uses `ConvState` directly (not separate `state` + `state_data`)
- [ ] DB migration handles existing conversations
- [ ] Sub-agent status persists correctly across page reloads
- [ ] All existing tests pass

## Implementation Plan

### 1. Schema Migration

```sql
-- Migrate existing data: combine state + state_data into single JSON
-- For most rows, state_data is NULL so just wrap the state string
ALTER TABLE conversations RENAME COLUMN state TO state_old;
ALTER TABLE conversations RENAME COLUMN state_data TO state_data_old;
ALTER TABLE conversations ADD COLUMN state TEXT NOT NULL DEFAULT '{"type":"idle"}';

-- Migration logic needed to convert state_old -> proper JSON
-- Then drop state_old and state_data_old columns
```

### 2. Update `src/db/schema.rs`

- Remove `ConversationState` enum (or alias to `ConvState`)
- Update `Conversation` struct:
  ```rust
  pub struct Conversation {
      // ...
      pub state: ConvState,  // Was: ConversationState + Option<Value>
      // Remove: state_data: Option<Value>
  }
  ```

### 3. Update `src/db.rs`

- Remove `parse_state()` function
- Update `get_conversation()` to deserialize JSON directly:
  ```rust
  state: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
  ```
- Update `update_conversation_state()` to serialize full state:
  ```rust
  let state_json = serde_json::to_string(&state)?;
  conn.execute(
      "UPDATE conversations SET state = ?1, ...",
      params![state_json, ...],
  )?;
  ```

### 4. Update `src/runtime/executor.rs`

- Simplify `Effect::PersistState` handler - no more manual extraction:
  ```rust
  Effect::PersistState => {
      self.storage
          .update_state(&self.context.conversation_id, &self.state)
          .await?;
      // Broadcast uses self.state directly
  }
  ```

### 5. Update `src/runtime/traits.rs`

- `Storage::update_state` takes `&ConvState` instead of `&ConversationState` + `Option<&Value>`

### 6. Update API responses

- `Conversation` JSON serialization now includes full state
- SSE `init` event gets state from `conversation.state` directly
- Remove any `state_data` references in API handlers

### 7. Update UI

- `static/app.js` init handler already extracts from `conversation.state` object
- Verify no references to separate `state_data` field

## Files to Modify

- `src/db/schema.rs` - Conversation struct, remove ConversationState enum
- `src/db.rs` - Remove parse_state, update queries
- `src/db/migrations/` - Add migration for schema change
- `src/runtime/executor.rs` - Simplify PersistState handler
- `src/runtime/traits.rs` - Update Storage trait
- `src/api/handlers.rs` - May need updates for response format
- `src/api/types.rs` - Response types
- `static/app.js` - Verify init handler works with new format

## Testing

- Existing state machine proptests should still pass
- Test page reload during `AwaitingSubAgents` - should preserve counts
- Test page reload during `ToolExecuting` - should preserve tool info
- Test migration on DB with existing conversations

## Notes

- `ConvState::Default` is `Idle`, so `unwrap_or_default()` is safe fallback
- Consider what happens if schema changes in future - may want version field
- The `to_db_state()` method can be removed or kept for logging only
