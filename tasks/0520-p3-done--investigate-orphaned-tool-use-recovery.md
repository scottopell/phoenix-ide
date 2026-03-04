---
created: 2025-02-07
priority: p3
status: done
---

# Investigate: Orphaned Tool Use Recovery

## Summary

Compare how rustey-shelley and phoenix-ide handle incomplete tool exchanges in conversation history after crashes/errors. Goal: make invalid states unrepresentable.

## The Bug in rustey-shelley

Commit `fb8615b` fixed a crash recovery bug:

**Scenario:**
1. LLM requests tool_use (bash command)
2. Server crashes mid-execution
3. On restart, history contains `tool_use` without `tool_result`
4. Claude API rejects the malformed history

**Fix:** `filter_complete_exchanges()` scans entire history and removes any assistant message with `tool_use` that isn't immediately followed by a user message with matching `tool_result`.

```rust
// From rustey-shelley src/agent/loop.rs
fn filter_complete_exchanges(history: &[LlmMessage]) -> Vec<LlmMessage> {
    // Scans for orphaned tool_use and removes them
    // Applied both to load_history() and continuation requests
}
```

## Investigation Tasks

### 1. Analyze phoenix-ide's current behavior

- [ ] What happens when phoenix-ide crashes mid-tool-execution?
- [ ] Is there history filtering when loading conversations?
- [ ] Search for: `filter`, `complete`, `orphan`, `tool_use`, `tool_result` in transition.rs, executor.rs
- [ ] Check `src/runtime/executor.rs` for history reconstruction

### 2. Identify the state machine handling

- [ ] Phoenix has `ToolExecuting` state - what's persisted when we're in this state?
- [ ] Is `tool_use` stored before or after execution completes?
- [ ] What state do we resume to after crash?

### 3. Test the crash scenario

- [ ] Start a conversation, trigger a slow bash command
- [ ] Kill the server mid-execution
- [ ] Restart and try to continue the conversation
- [ ] Does it error? Does it recover gracefully?

### 4. Pit of Success Analysis

Can we make this state unrepresentable? Options:

1. **Transactional persistence:** Only persist tool_use + tool_result together
2. **Synthetic error results:** On recovery, inject error tool_result for any orphaned tool_use
3. **History validation:** Filter on load (rustey-shelley approach)
4. **State machine invariant:** Ensure tool_use is never persisted without result

## Reference Files

**rustey-shelley:**
- `src/agent/loop.rs` - `filter_complete_exchanges()`, `load_history()`
- Commit `fb8615b` - the fix

**phoenix-ide:**
- `src/state_machine/transition.rs` - tool execution states
- `src/runtime/executor.rs` - runtime execution
- `src/db.rs` - persistence layer

## Success Criteria

- Document exactly how phoenix-ide handles this case
- If vulnerable, propose a fix that makes the invalid state unrepresentable
- Add test case for crash-during-tool-execution recovery

---

## Investigation Findings (2025-02-07)

### Summary

**Phoenix-IDE IS VULNERABLE to the orphaned tool_use bug.** There is no filtering of incomplete tool exchanges.

### Analysis

#### Current Behavior

1. **Startup Reset (`main.rs:60`):**
   ```rust
   db.reset_all_to_idle()?;
   ```
   On server restart, ALL conversations are reset to `Idle` state.

2. **Runtime Resume (`runtime.rs:335`):**
   ```rust
   ConvState::Idle, // Always resume from idle (REQ-BED-007)
   ```
   Conversations always resume from Idle, ignoring persisted state.

3. **No History Filtering (`executor.rs:588-641`):**
   `build_llm_messages_static()` reads ALL messages from DB and converts them directly to LLM format with no validation for complete tool exchanges.

#### The Vulnerability

Crash scenario:
1. User sends message → LLM returns `tool_use`
2. Agent message (with `tool_use` block) is persisted via `Effect::persist_agent_message`
3. State transitions to `ToolExecuting`, `Effect::PersistState` saves state
4. `Effect::execute_tool` starts tool execution
5. **SERVER CRASHES** mid-execution
6. On restart, `reset_all_to_idle()` sets state to `Idle`
7. User sends another message
8. `build_llm_messages_static` loads history:
   - User message
   - Agent message with `tool_use`
   - **NO `tool_result`** (never persisted - tool didn't complete)
   - New user message
9. **Claude API rejects** - tool_use without matching tool_result is invalid

#### Key Code Paths

**Message persistence order in `transition.rs` (lines 145-166):**
```rust
// LlmResponse with tools -> ToolExecuting
Ok(TransitionResult::new(ConvState::ToolExecuting { ... })
    .with_effect(Effect::persist_agent_message(content, ...))  // tool_use persisted HERE
    .with_effect(Effect::PersistState)
    .with_effect(Effect::execute_tool(first)))  // tool execution starts AFTER persist
```

The agent message (containing `tool_use`) is persisted BEFORE tool execution begins. This creates the window for orphaned tool_use.

### Comparison with rustey-shelley

rustey-shelley's fix (`src/agent/loop.rs`):
```rust
pub(crate) fn filter_complete_exchanges(history: &[LlmMessage]) -> Vec<LlmMessage> {
    // Scans history, removes any assistant message with tool_use
    // that isn't immediately followed by user message with matching tool_result
}
```

Called in two places:
1. `load_history()` - when loading conversation from DB
2. Before continuation requests

### Recommended Fix Options

#### Option 1: Filter on Load (Matches rustey-shelley)
Add `filter_complete_exchanges()` to `build_llm_messages_static()`.
- **Pros:** Simple, proven approach
- **Cons:** Silently drops orphaned messages (may confuse users)

#### Option 2: Synthetic Error Results on Recovery
On startup, scan for conversations with `ToolExecuting` state, inject synthetic `tool_result` with error message.
- **Pros:** Maintains history integrity, user sees what happened
- **Cons:** More complex, requires startup scan

#### Option 3: Transactional Persistence
Only persist `tool_use` and `tool_result` together after tool completes.
- **Pros:** Makes invalid state unrepresentable
- **Cons:** Significant architecture change, loses real-time visibility of tool execution

### Solution Implemented

**Option 2 (Synthetic Error Results on Startup)** was implemented.

Key insight: Crashes are outside our control, but **startup is within our control**. We repair orphans before the system resumes, so from the LLM's perspective, invalid states are never observable.

#### Implementation

`reset_all_to_idle()` in `src/db.rs` now:
1. Scans all conversations for orphaned `tool_use` blocks
2. Injects synthetic `tool_result` with message: "Tool execution interrupted by server restart"
3. Then resets all conversations to Idle

```rust
fn repair_orphaned_tool_use_internal(&self, conn: &Connection, now: &DateTime<Utc>) -> DbResult<()> {
    // For each conversation:
    //   - Track pending tool_use IDs from agent messages
    //   - Remove IDs when matching tool_result found
    //   - Inject synthetic result for any remaining orphans
}
```

#### Why This Approach

1. **Preserves LLM context** - Agent message was persisted, so LLM remembers what it attempted
2. **Valid API state** - Synthetic result completes the exchange
3. **User visibility** - Can see what was interrupted
4. **Structural guarantee** - Invalid state exists momentarily after crash but is repaired before anything reads it

#### Tests Added

- `test_reset_repairs_orphaned_tool_use` - Single orphan repaired
- `test_reset_does_not_duplicate_complete_exchanges` - Complete exchanges untouched
- `test_reset_repairs_multiple_orphaned_tools` - Multiple orphans in one agent message

### Checklist Update

- [x] What happens when phoenix-ide crashes mid-tool-execution? → **Now repaired on startup**
- [x] Is there history filtering when loading conversations? → **Not needed - orphans repaired**
- [x] Phoenix has `ToolExecuting` state - what's persisted? → **Agent message persisted immediately (preserved)**
- [x] Is `tool_use` stored before or after execution completes? → **Before (for LLM context)**
- [x] What state do we resume to after crash? → **Idle, with repaired history**
- [x] Implement fix → **Done in `reset_all_to_idle()`**
