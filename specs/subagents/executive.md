# Sub-Agents - Executive Summary

## Requirements Summary

Sub-agents enable parallel task execution by spawning independent child
conversations that run concurrently and report results back to a parent
conversation. Each sub-agent runs in isolation and cannot spawn its own
sub-agents. The parent specifies mode (explore for read-only research,
work for write access) and optionally a worker, execution selector, and turn budget per
sub-agent. Mode enforcement rejects Work sub-agent requests from Explore parents
when such requests are received. Top-level Explore always exposes `spawn_agents`;
process-wide sandbox support gates only whether Explore parents and spawned
Explore sub-agents receive sandboxed bash. Without sandbox support, delegation
still works with read/browser/submit tools and no bash. Work, Branch, and Direct
parents can spawn either. The normative target qualifies the parent's resolved
model by exact identifier: GPT-5.6 Sol/Terra, Astra, and GPT-6 Sol may admit parallel Work children;
Luna and every unlisted model remain sequential. Child execution choices do not
change that decision. Admitted Work children intentionally share the parent's
exact `WorkScope` as trusted collaborators. When the parent owns a worktree (Work
or Branch mode), a Work sub-agent's effective cwd — including any `task.cwd`
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
  when all tools complete. The normative target atomically admits complete
  batches, durably tracks each admitted child, removes the exact pending identity
  on terminal acceptance, and makes duplicate acceptance idempotent. Cancellation
  includes admitted children that have not started; ordinary settlement waits for
  complete fan-in.
- **Sub-agent terminal states** are `completed { result }` and
  `failed { error, error_kind }`. The `submit_result` / `submit_error`
  tools must be the sole tool in their LLM response; the transition
  function enforces this structurally.
- **Spawn-layer** (`tools/subagent.rs` + `runtime/executor.rs::
  handle_spawn_agents_tool`) validates the call, applies defaults
  (mode, model, max_turns, cwd, timeout), enforces exact parent-model
  qualification plus cwd scoping, then hands each task to
  `RuntimeManager::handle_spawn_request`. `runtime.rs` derives the
  sub-agent's `ConvMode` from the parent's mode and selects the
  per-mode tool registry (`for_subagent_explore` /
  `for_subagent_work`); on runtime re-creation the registry is
  recovered from the persisted `conv_mode`.
- **Timeout** is a 20-minute wall-clock safety-net set when the parent
  enters `awaiting_sub_agents`; `max_turns` (per-mode default 20/50) is
  the primary budget.
- **Grace dispatch** narrows the callable request surface to
  `submit_result` / `submit_error` while retaining completed ordinary-tool
  history for synthesis. The admission guard remains a malformed-response
  backstop.
- **Spawn execution** uses an optional tier or exact model/connection selector,
  independently of an optional named worker. Worker candidates provide defaults;
  generic omission inherits the parent's model, connection, and effort. Exact
  model omission of effort uses its native default. Request schema and admission
  share a resolved catalog. Selected connection identity is persisted for runtime
  recreation without adding server-restart survival guarantees.
- **Path defaults** treat blank cwd overrides as absent and resolve relative cwd
  values from the parent conversation.
- **Sub-agent wake handle** is the child conversation / agent id. Wake contracts
  can wait on that handle reaching terminal state, but the wake payload is not a
  parent-to-child continuation channel and does not grant more budget.
- **Named workers** supply persona and execution preferences, never mode or tools.
  The XDG config and candidate catalog are owned by
  [`agents.allium`](../agents/agents.allium); [ADR-052](../adrs/052_workers-and-tiers-resolve-usable-model-routes.md)
  records the selection and filesystem retirement decisions.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-SA-001:** Parallel Task Execution | 🚧 Partial | Existing Explore parallelism and bounds are live; atomic qualified Work batch admission is normative but not implemented |
| **REQ-SA-002:** Sub-Agent Isolation | ✅ Complete | Tool registries exclude `spawn_agents`, `ask_user_question`, `skill`, `propose_task`; sub-agents tagged `user_initiated = false` |
| **REQ-SA-003:** Result Submission | 🚧 Partial | Terminal tools are live; durable parent-identity delivery and idempotent acceptance remain to be implemented |
| **REQ-SA-004:** Parent Fan-In | 🚧 Partial | Existing fan-in is live; exact durable pending removal, idempotent duplicate acceptance, and settlement fencing remain to be implemented |
| **REQ-SA-005:** Cancellation Propagation | 🚧 Partial | Installed-child cancellation is live; durable cancel-before-start and joined materialization remain to be implemented |
| **REQ-SA-006:** Timeout Enforcement | ✅ Complete | `DEFAULT_SUBAGENT_TIMEOUT = 20 min`; deadline races in executor `select!` |
| **REQ-SA-007:** Model Selection | ✅ Complete | `generic_omission_inherits_parent_execution`, `override_replaces_execution_and_keeps_persona`, and `explicit_connection_is_exact_and_never_falls_back`; persisted selection verified by `unattached_sub_agent_persists_selection_without_parent_effort_leak` |
| **REQ-SA-008:** Context Injection via Read-First | ❌ Not Started | `read_first` field not yet on `SubAgentTask`; deferred |
| **REQ-SA-009:** Terminal Handle Identity for Wake Contracts | Proposed | Child conversation / agent id is the sub-agent wake handle |
| **REQ-SA-010:** Turn-Limit Grace Prompt Integrity | ✅ Complete | Grace request advertises terminal tools only; Work guidance routes unfinished required edits through `submit_error` |
| **REQ-SA-011:** Spawn Override Defaults and Path Base | ✅ Complete | `omitted_execution_and_blank_cwd_use_defaults`, `relative_cwd_resolves_from_parent_working_directory`, and `advertised_model_removed_from_live_registry_is_rejected` |

**Progress:** 5 complete; 4 partial; 1 deferred; 1 proposed for wake runtime.

## Execution-selection verification

Validation on devmbp passed the full Rust suite, compilation, code generation,
generated-file staleness, Allium, spec shape, spec anchors, end-to-end tests, and
dev.py tests at `caab8290d`. `unknown_model_on_later_task_rejected_before_any_spawn`
verifies batch admission before dispatch; `sub_agent_selection_failure_rolls_back_conversation_and_persona`
verifies atomic child persistence.

## Deferred refinements

- **Explore-MCP subset:** Explore sub-agents currently receive the
  parent's full MCP tool set. A search-restricted subset (Atlassian
  search, Google Workspace search, ...) is a documented deferred
  refinement — kept deferred per task 13010. The spec records the
  current behaviour; a future task can promote it.
- **REQ-SA-008 `read_first`:** Not yet on the wire-level
  `SubAgentTask`. Tracked in this status table.
