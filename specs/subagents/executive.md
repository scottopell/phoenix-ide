# Sub-Agents - Executive Summary

## Requirements Summary

Sub-agents enable parallel task execution by spawning independent child
conversations that run concurrently and report results back to a parent
conversation. Each sub-agent runs in isolation and cannot spawn its own
sub-agents. The parent specifies mode (explore for read-only research,
work for write access) and optionally a model and turn budget per
sub-agent. Mode enforcement rejects Work sub-agent requests from Explore parents
when such requests are received. Top-level Explore always exposes `spawn_agents`;
process-wide sandbox support gates only whether Explore parents and spawned
Explore sub-agents receive sandboxed bash. Without sandbox support, delegation
still works with read/browser/submit tools and no bash. Work, Branch, and Direct
parents can spawn either, with at
most one Work sub-agent active at a time per parent (and per
`spawn_agents` call). When the parent owns a worktree (Work or Branch
mode), a Work sub-agent's effective cwd — including any `task.cwd`
override — must stay inside that worktree; a Work sub-agent spawned
from a Direct parent has no worktree to scope against, matching
Direct's unscoped write semantics. Results are submitted via dedicated
tools (`submit_result` / `submit_error`). Maximum 10 sub-agents per
spawn call.

## Technical Summary

The detailed state-machine and spawn-layer behaviour is normative in
[`subagents.allium`](./subagents.allium) +
[`bedrock.allium`](../bedrock/bedrock.allium); this section summarises
only the architectural seams.

- **State machine** lives in bedrock: `executing_tools` accumulates
  `pending_sub_agents`; the parent transitions to `awaiting_sub_agents`
  when all tools complete; fan-in uses a bounded buffer (capacity = the
  spawn batch size) for results that arrive before the parent enters the
  await state. Cancellation flows through `cancelling_sub_agents` and
  back to idle.
- **Sub-agent terminal states** are `completed { result }` and
  `failed { error, error_kind }`. The `submit_result` / `submit_error`
  tools must be the sole tool in their LLM response; the transition
  function enforces this structurally.
- **Spawn-layer** (`tools/subagent.rs` + `runtime/executor.rs::
  handle_spawn_agents_tool`) validates the call, applies defaults
  (mode, model, max_turns, cwd, timeout), enforces the one-writer +
  cwd-scoping invariants, then hands each task to
  `RuntimeManager::handle_spawn_request`. `runtime.rs` derives the
  sub-agent's `ConvMode` from the parent's mode and selects the
  per-mode tool registry (`for_subagent_explore` /
  `for_subagent_work`); on runtime re-creation the registry is
  recovered from the persisted `conv_mode`.
- **Timeout** is a 20-minute wall-clock safety-net set when the parent
  enters `awaiting_sub_agents`; `max_turns` (per-mode default 20/50) is
  the primary budget.
- **Named agents** (see [`../agents/`](../agents/executive.md)) thread
  through this layer: `SubAgentTask` gains an optional `agent_type`;
  `SubAgentSpec` gains `agent_name` and `persona`;
  `SpawnRejectedUnknownAgentType` rejects an unmatched `agent_type`; and
  `SubAgentSpecsResolved` resolves mode/model with the agent definition as
  the middle precedence layer. Persona discovery and composition are owned
  by `agents.allium`.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-SA-001:** Parallel Task Execution | ✅ Complete | Mode/model/max-turns wired; max 10 tasks per call |
| **REQ-SA-002:** Sub-Agent Isolation | ✅ Complete | Tool registries exclude `spawn_agents`, `ask_user_question`, `skill`, `propose_task`; sub-agents tagged `user_initiated = false` |
| **REQ-SA-003:** Result Submission | ✅ Complete | `submit_result` / `submit_error`; terminal-tool-must-be-sole enforced structurally |
| **REQ-SA-004:** Parent Fan-In | ✅ Complete | Bounded buffer; conservation invariant tested in proptests |
| **REQ-SA-005:** Cancellation Propagation | ✅ Complete | `cancelling_sub_agents` state, propagates `UserCancel`; missing-runtime synthesises failure |
| **REQ-SA-006:** Timeout Enforcement | ✅ Complete | `DEFAULT_SUBAGENT_TIMEOUT = 20 min`; deadline races in executor `select!` |
| **REQ-SA-007:** Model Tier Selection | ✅ Superseded | Replaced by REQ-PROJ-008 mode defaults + explicit `model` override |
| **REQ-SA-008:** Context Injection via Read-First | ❌ Not Started | `read_first` field not yet on `SubAgentTask`; deferred |

**Progress:** 7 of 8 implemented (one explicitly superseded; one deferred).

## Deferred refinements

- **Explore-MCP subset:** Explore sub-agents currently receive the
  parent's full MCP tool set. A search-restricted subset (Atlassian
  search, Google Workspace search, ...) is a documented deferred
  refinement — kept deferred per task 13010. The spec records the
  current behaviour; a future task can promote it.
- **REQ-SA-008 `read_first`:** Not yet on the wire-level
  `SubAgentTask`. Tracked in this status table.
