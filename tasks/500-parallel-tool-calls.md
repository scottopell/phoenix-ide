---
created: 2026-02-07
priority: p2
status: brainstorming
---

# Parallel Tool Calls

## Summary

Investigate and design support for executing multiple LLM-requested tool calls in parallel, rather than the current serial execution model.

## Motivation / Observed Behavior

Claude Code 2.1.37 demonstrates parallel tool execution:

```
⏺ Fetch(https://dl01.fedoraproject.org/pub/fedora/linux/releases/41/Cloud/aarch64/images/)
  ⎿  Error: Request failed with status code 404

⏺ Fetch(https://dl01.fedoraproject.org/pub/fedora/linux/releases/41/Cloud/x86_64/images/)
  ⎿  Error: Sibling tool call errored

⏺ Fetch(https://dl01.fedoraproject.org/pub/fedora/linux/releases/42/Cloud/aarch64/images/)
  ⎿  Fetching…

⏺ Fetch(https://dl01.fedoraproject.org/pub/fedora/linux/releases/42/Cloud/x86_64/images/)
  ⎿  Fetching…
```

Key observations:
1. Multiple Fetch calls execute simultaneously ("Fetching..." in parallel)
2. "Sibling tool call errored" suggests coordinated error handling across parallel calls
3. Errors in one call may affect behavior of sibling calls

## Current State Machine

### Relevant States

```rust
ToolExecuting {
    current_tool: ToolCall,           // Single tool being executed
    remaining_tools: Vec<ToolCall>,   // Queue of remaining tools (serial)
    persisted_tool_ids: HashSet<String>,
    pending_sub_agents: Vec<String>,
}
```

### Relevant Events

```rust
Event::ToolComplete {
    tool_use_id: String,
    result: ToolResult,
}

Event::ToolAborted {
    tool_use_id: String,
}
```

### Current Transition Flow (Serial)

1. `LlmRequesting` + `LlmResponse{tools}` → `ToolExecuting{current=first, remaining=rest}`
2. `ToolExecuting` + `ToolComplete` (more remaining) → `ToolExecuting{current=next, remaining=rest-1}`
3. `ToolExecuting` + `ToolComplete` (none remaining) → `LlmRequesting`

### Invariants

- Each `tool_use_id` is persisted exactly once (validated by `persisted_tool_ids`)
- Tool results are persisted in-order as they complete
- All tools must have results before next LLM request
- Cancellation generates synthetic results for in-flight + skipped tools

---

## Problem Statements

### P1: When Should Tools Execute in Parallel?

**Classification Problem:** Not all tool calls can/should be parallelized.

| Tool | Parallelizable? | Reasoning |
|------|-----------------|------------|
| `bash` | Maybe | Side effects (file system, env vars) may conflict |
| `patch` | No | Must apply in sequence to same file |
| `think` | Yes | Pure computation, no side effects |
| `keyword_search` | Yes | Read-only query |
| `read_image` | Yes | Read-only I/O |
| `spawn_agents` | Yes | Independent sub-agent tasks |
| `fetch` (external) | Yes | Independent network requests |

**Open Questions:**
- Does the LLM signal parallelizability, or do we infer it?
- Can we detect conflicting operations (e.g., two patches to same file)?
- Should we have tool-level metadata: `{ parallelizable: bool, conflicts_with: [paths] }`?

### P2: State Machine Representation

**Current:** Single `current_tool` assumes serial execution.

**Needed:** Track multiple in-flight tools + their completion status.

```rust
// Option A: Set-based
ToolExecuting {
    executing_tools: HashSet<String>,  // IDs currently in-flight
    pending_tools: Vec<ToolCall>,       // Not yet started
    completed_results: Vec<ToolResult>, // Results waiting for commit
}

// Option B: Map-based (tracks full state per tool)
ToolExecuting {
    tools: HashMap<String, ToolExecState>,  // id -> Pending | Running | Complete(result)
}
```

**Questions:**
- How do we represent partial completion?
- When do we commit results to DB: immediately, or batched when all complete?
- How does this interact with `persisted_tool_ids` validation?

### P3: Completion Semantics

**Serial model:** Complete tool A → start tool B → complete tool B → start tool C...

**Parallel model options:**

1. **All-or-nothing:** Wait for all parallel tools to complete, then proceed
   - Simpler state transitions
   - Delays LLM on slowest tool

2. **First-failure-aborts:** On any error, abort remaining parallel siblings
   - Claude Code's "Sibling tool call errored" suggests this
   - Need synthetic results for aborted siblings

3. **Independent completion:** Tools complete independently, LLM sees partial results
   - Most complex
   - Unclear if Claude API supports this

**Questions:**
- What does Claude API expect for parallel tool results?
- Can we send `tool_result` messages out-of-order?
- Does the LLM handle partial success gracefully?

### P4: Cancellation Complexity

**Serial cancellation is already complex:**

```rust
CancellingTool {
    tool_use_id: String,         // Being aborted
    skipped_tools: Vec<ToolCall>, // Never started
    persisted_tool_ids: HashSet<String>,  // Already complete
}
```

**Parallel cancellation adds:**
- Multiple tools simultaneously being aborted
- Race between abort request and completion
- Coordinating synthetic results for partial completion

**Questions:**
- Do we abort all parallel siblings, or just the "current group"?
- How do we handle tool that completes while we're aborting its sibling?
- Is there a priority order for which results to keep?

### P5: Error Handling Coordination

**Observed behavior:** "Sibling tool call errored" suggests error propagation.

**Design questions:**
- Should one tool's error immediately abort siblings?
- Should we collect all errors and present together?
- How do we attribute errors (which sibling failed)?
- Should we retry failed tools independently?

### P6: UI/UX for Parallel Execution

**Current:** Single "executing tool X" indicator.

**Parallel needs:**
- Show multiple in-flight tools
- Progress for each independently
- Error states per-tool
- Collapse/expand tool groups?

**Questions:**
- How does `notify_tool_executing` change?
- Do we need new SSE event types?
- How do we order tools in the UI when they complete out-of-order?

### P7: Database/Persistence Model

**Current:** Messages persisted serially via `Effect::PersistMessage`.

**Questions:**
- Do parallel tool results need sequential message ordering?
- How do we handle DB write failures for one tool in a parallel batch?
- Should we use transactions for parallel tool persistence?

---

## Potential Design Solutions

### Solution 1: Parallel Batches with Barrier

Group tools into sequential "batches", where tools within a batch execute in parallel.

```rust
ToolExecuting {
    // Current parallel batch
    current_batch: ParallelBatch {
        tools: HashMap<String, ToolExecState>,
    },
    // Remaining sequential batches
    remaining_batches: Vec<Vec<ToolCall>>,
    persisted_tool_ids: HashSet<String>,
}

enum ToolExecState {
    Running,
    Completed(ToolResult),
    Aborted { reason: String },
}
```

**Transition:** Batch completes when ALL tools reach terminal state → persist all → start next batch or `LlmRequesting`.

**Pros:** Clear synchronization points, predictable ordering.
**Cons:** Adds batching complexity, may not match LLM's expectations.

### Solution 2: LLM-Driven Parallelization Hints

Extend the tool call parsing to detect LLM-provided parallelization signals.

```rust
struct ToolCall {
    id: String,
    input: ToolInput,
    // LLM-provided hint (e.g., from thinking block or special field)
    parallel_group: Option<String>,
}
```

Tools with same `parallel_group` execute together; different groups are sequential barriers.

**Pros:** LLM controls scheduling, adapts to task semantics.
**Cons:** Requires LLM cooperation, unclear if Claude supports this.

### Solution 3: Conservative Inference

Automatically parallelize "obviously safe" tools based on tool type.

```rust
impl ToolInput {
    fn is_safe_for_parallel(&self) -> bool {
        matches!(self,
            ToolInput::Think(_) |
            ToolInput::KeywordSearch(_) |
            ToolInput::ReadImage(_)
        )
    }
}
```

Partition tool list into: `[safe-parallel-batch, unsafe-serial, safe-parallel-batch, ...]`

**Pros:** No LLM changes needed, safe default.
**Cons:** May miss parallelization opportunities, heuristics can be wrong.

---

## Use Cases to Support

1. **Multiple file reads:** `read_image(a.png)` + `read_image(b.png)` + `read_image(c.png)`
2. **Multiple searches:** `keyword_search(foo)` + `keyword_search(bar)`
3. **Mixed parallelizable:** `think(plan)` + `bash(ls)` - should `think` wait for `bash`?
4. **Network fetches:** Multiple URL fetches (not currently a tool, but motivating example)
5. **Sub-agent spawning:** Already somewhat parallel via `spawn_agents`

---

## Open Research Questions

1. **Claude API behavior:** Does Claude expect tool results in order? Does it use `parallel_tool_use` parameter?
2. **Claude Code implementation:** How does Claude Code 2.1.37 implement this internally?
3. **Anthropic guidance:** Any documentation on parallel tool execution best practices?

---

## Next Steps

- [ ] Research Claude API parallel tool use capabilities
- [ ] Prototype parallel execution for read-only tools only
- [ ] Design `notify_tool_executing` changes for parallel UI
- [ ] Write property tests for parallel completion scenarios
- [ ] Analyze cancellation edge cases in detail

---

## References

- `src/state_machine/transition.rs` - Current serial tool execution logic
- `src/state_machine/state.rs` - `ToolExecuting` state definition
- `STATE_MACHINE_EVALUATION.md` - State machine design principles
- Claude Code 2.1.37 observed behavior (see Motivation section)
