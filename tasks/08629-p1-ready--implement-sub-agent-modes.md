---
created: 2026-04-06
priority: p1
status: ready
artifact: src/tools/subagent.rs
---

# Implement sub-agent modes (REQ-PROJ-008)

## Summary

Sub-agents currently have one mode: same restricted tool set for all, inherit
parent model, no MCP access, no turn limits, no concept of explore vs work.
This task adds mode-aware sub-agents per the design spec.

Use the /serial-sub-agent skill to decompose and delegate this work. The scope
is large (6 concerns across ~8 files) but each concern is self-contained.

## Design Reference

Read `specs/projects/design.md`, section "Work Sub-Agent Mode Inheritance"
(REQ-PROJ-008). It covers: tool schema, tool registries per mode, model
selection, one-writer constraint, MCP access, max turns, and working directory
assignment. Follow it as the source of truth for behavior.

## Key Files

- `src/tools/subagent.rs` -- SpawnAgentsTool schema and SpawnAgentsInput struct
- `src/tools.rs` -- ToolRegistry constructors (for_subagent, explore_no_sandbox, etc.)
- `src/runtime.rs` -- Sub-agent creation (start_sub_agent_handler), model selection
- `src/runtime/executor.rs` -- Sub-agent spawning, timeout/deadline, turn counting
- `src/runtime/traits.rs` -- ToolExecutor trait, ToolRegistryExecutor (MCP integration)
- `src/state_machine/state.rs` -- SubAgentSpec, ConvContext

## What to Build

**Schema changes (subagent.rs):** Add `mode` (optional, "explore" or "work"),
`model` (optional string), and `max_turns` (optional u32) to the spawn_agents
input schema and the `SubAgentTaskSpec` / `SpawnAgentsInput` struct.

**Tool registries (tools.rs):** Create `ToolRegistry::for_subagent_explore()`
and `ToolRegistry::for_subagent_work()`. Explore gets: think, bash, keyword_search,
read_image, browser tools, submit_result, submit_error. NO patch, NO spawn_agents,
NO ask_user_question, NO skill, NO propose_task. Work gets: everything Explore
has PLUS bash (write-enabled) and patch.

**Read-only bash is a stub.** Explore sub-agents should get BashTool (the regular
one). Add a comment: `// TODO: read-only bash enforcement not yet implemented --
// uses regular bash. See REQ-BASH-008 for the planned sandbox approach.`
This gives us the right code path without blocking on sandbox implementation.

**Model selection (runtime.rs):** When creating a sub-agent, resolve the model:
if task spec has `model`, use it. Otherwise, explore defaults to the registry's
haiku model (look up by name pattern), work inherits the parent's model_id. Pass
the resolved model to ConvContext.

**MCP access (runtime.rs, traits.rs):** Currently sub-agents get
`ToolRegistryExecutor::builtin_only()` which has no MCP. Change to pass the
parent's MCP client manager so sub-agents get deferred MCP tools via tool search.
Both explore and work modes get MCP access.

**One-writer constraint (runtime.rs, executor.rs):** Track active Work sub-agents
per parent conversation. Use an AtomicU32 or simple counter on the runtime handle.
Reject spawn if a Work sub-agent is already active. Decrement on completion/failure.
Multiple Explore sub-agents are always allowed.

**Max turns (executor.rs):** Add a turn counter to the executor. Each LlmRequesting
transition increments it. When the count exceeds the limit (explore=20, work=50,
overridable via task spec), inject a forced submit_error("Reached maximum turn
limit"). The existing 5-minute timeout remains as a secondary safety net.

**Default mode resolution:** Parent in Explore -> sub-agents default Explore,
Work rejected. Parent in Work -> sub-agents default Explore, Work available.
Parent in Standalone -> sub-agents default Explore.

## Done When

- [ ] spawn_agents accepts mode, model, max_turns per task
- [ ] Explore sub-agents get read-only tools (stub) + haiku default
- [ ] Work sub-agents get full tools + parent model
- [ ] Only one Work sub-agent per parent at a time (enforced at spawn)
- [ ] MCP tools available to sub-agents via tool search
- [ ] Max turns enforced, forced completion on limit
- [ ] `./dev.py check` passes (all 8 checks)
