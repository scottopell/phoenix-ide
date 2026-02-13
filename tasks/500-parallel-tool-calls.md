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

**Confirmed:** The LLM does NOT signal parallelizability - this is entirely our decision as the tool executor. Options:

1. **Tool-level metadata:** `impl ToolInput { fn is_parallelizable(&self) -> bool }`
2. **Conflict detection:** Analyze tool inputs for overlapping resources (e.g., same file paths)
3. **Conservative default:** Only parallelize obviously-safe tools (think, keyword_search, read_image)

**Open Questions:**
- Can we detect conflicting operations (e.g., two patches to same file)?
- Should `bash` ever be parallelized? (race conditions, shared state)
- How granular should conflict detection be? (file-level? directory-level?)

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
   - ~~Unclear if Claude API supports this~~ **Confirmed supported** - APIs accept results in any order

**API Findings:** Both Anthropic and OpenAI APIs accept tool results in any order. Each result is matched by `tool_use_id`, so ordering is irrelevant. This means option 3 (independent completion) is fully viable from an API perspective.

**Remaining Questions:**
- What's the best UX? Show results as they arrive, or wait for batch?
- How should errors in one tool affect display of siblings?

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

### ~~Solution 2: LLM-Driven Parallelization Hints~~ (Not Viable)

**Ruled out by API research.** Neither Anthropic nor OpenAI APIs provide parallelization hints or grouping signals. The LLM simply emits tool calls; execution strategy is entirely client-side.

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

## API Research Findings

### Key Insight: Execution Order is Client's Decision

Neither Anthropic nor OpenAI APIs prescribe execution order. They simply:
1. Return N tool calls in a single response
2. Expect N tool results back before the next turn
3. Match results to calls via unique `tool_use_id`

**Tool results can be returned in any order** - the APIs don't care about ordering.

### Anthropic API

**Request structure:**
```json
{
  "content": [
    { "type": "tool_use", "id": "call_1", "name": "bash", "input": {...} },
    { "type": "tool_use", "id": "call_2", "name": "bash", "input": {...} },
    { "type": "text", "text": "Let me check both..." }
  ]
}
```

**Response with results:**
```json
{
  "role": "user",
  "content": [
    { "type": "tool_result", "tool_use_id": "call_2", "content": "..." },
    { "type": "tool_result", "tool_use_id": "call_1", "content": "..." }
  ]
}
```

Note: Results sent in reverse order - API accepts this.

**`parallel_tool_use` parameter:** Controls whether the *model* can emit multiple tool calls in one turn. Does NOT affect execution - that's entirely client-side.

### OpenAI Chat Completions API

**Response structure:**
```json
{
  "choices": [{
    "message": {
      "tool_calls": [
        { "id": "call_1", "function": { "name": "bash", "arguments": "..." } },
        { "id": "call_2", "function": { "name": "bash", "arguments": "..." } }
      ]
    }
  }]
}
```

**Submitting results:** Each tool result is a separate message with `role: "tool"` and `tool_call_id`. Order doesn't matter.

### OpenAI Responses API (Codex models)

Uses `function_call` outputs and `function_call_output` inputs. Same principle - results matched by `call_id`, order irrelevant.

### Implications for Design

1. **Solution 2 (LLM-Driven Hints) is not viable** - APIs don't provide parallelization signals
2. **P3 (Completion Semantics)** - "Independent completion" is fully supported by APIs
3. **P1 (Classification)** - Must be solved client-side, either via:
   - Tool metadata (`is_parallelizable()`)
   - Runtime conflict detection
   - Conservative heuristics

---

## Open Research Questions

1. ~~**Claude API behavior:** Does Claude expect tool results in order?~~ **ANSWERED:** No, order doesn't matter.
2. ~~**Does it use `parallel_tool_use` parameter?**~~ **ANSWERED:** Controls model output, not execution.
3. **Claude Code implementation:** How does Claude Code 2.1.37 implement parallel execution internally?
4. **Conflict detection:** Can we statically detect conflicting operations (e.g., two patches to same file)?
5. **Error propagation:** What's the best UX for "sibling tool call errored"?

---

## Next Steps

- [x] Research Claude API parallel tool use capabilities
- [ ] Prototype parallel execution for read-only tools only
- [ ] Design `notify_tool_executing` changes for parallel UI
- [ ] Write property tests for parallel completion scenarios
- [ ] Analyze cancellation edge cases in detail
- [ ] Add `is_parallelizable()` method to `ToolInput` enum
- [ ] Design conflict detection for `bash` and `patch` tools

---

## References

- `src/state_machine/transition.rs` - Current serial tool execution logic
- `src/state_machine/state.rs` - `ToolExecuting` state definition
- `STATE_MACHINE_EVALUATION.md` - State machine design principles
- Claude Code 2.1.37 observed behavior (see Motivation section)
