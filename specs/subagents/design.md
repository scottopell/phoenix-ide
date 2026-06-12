# Sub-Agents: Design Document

## Overview

Sub-agents enable parallel task execution by spawning independent child
conversations that run concurrently and report results back to a parent
conversation.

**Requirements:** REQ-BED-008 (Sub-Agent Spawning), REQ-BED-009 (Sub-Agent
Isolation), REQ-PROJ-008 (Sub-Agent Mode + Resource Controls).

> Detailed behaviour — states, transitions, invariants, mode rules,
> one-writer constraint, cwd-scoping, model/turn defaulting — is normative
> in [`subagents.allium`](./subagents.allium) (spawn layer) and
> [`bedrock.allium`](../bedrock/bedrock.allium) (state-machine layer).
> This file keeps only the architectural overview, the rationale, and
> example use cases.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      PARENT CONVERSATION                         │
│                                                                  │
│  ToolExecuting ───[spawn_agents]───▶ AwaitingSubAgents           │
│                                             │                    │
│       ┌─────────────────────────────────────┤ (SpawnSubAgent     │
│       │               │               │       effects)           │
│       ▼               ▼               ▼                          │
│  ┌─────────┐    ┌─────────┐    ┌─────────┐                       │
│  │SubAgent1│    │SubAgent2│    │SubAgent3│  (independent)        │
│  │  ...    │    │  ...    │    │  ...    │                       │
│  │Completed│    │ Failed  │    │Completed│  (terminal states)    │
│  └────┬────┘    └────┬────┘    └────┬────┘                       │
│       │              │              │                            │
│       │    SubAgentResult events    │                            │
│       └──────────────┼──────────────┘                            │
│                      ▼                                           │
│              AwaitingSubAgents ────▶ LlmRequesting               │
│              (all results collected)                             │
└─────────────────────────────────────────────────────────────────┘
```

Two layers cooperate:

1. **Spawn layer** — `crates/phoenix-ide/src/tools/subagent.rs` (the
   `spawn_agents` / `submit_result` / `submit_error` tools) and
   `crates/phoenix-ide/src/runtime/executor.rs::handle_spawn_agents_tool`
   (validation, defaulting, one-writer + cwd-scoping guards). Normative
   in `subagents.allium`.

2. **State-machine layer** — `crates/phoenix-ide/src/state_machine/`
   (the `executing_tools` / `awaiting_sub_agents` / `cancelling_sub_agents`
   / `completed` / `failed` transitions, the `SubAgentResult` fan-in,
   cancellation propagation). Normative in `bedrock.allium`.

3. **Cross-spec seam** — projects.allium owns the worktree contract
   (`WorkSubAgentInheritsWorktree`, `ExploreSubAgentDirectory`) and the
   universal working-directory root floor: a conversation or sub-agent cwd
   must canonicalise to an existing non-root directory before it is persisted
   or used to build a runtime context. The cwd-scoping invariant in
   `subagents.allium` additionally keeps Work sub-agent writes inside the
   parent's worktree boundary.

4. **Named-agent seam** — [`agents.allium`](../agents/agents.allium) owns
   agent discovery, the `agent_type` schema enum, and persona composition.
   This spec owns the spawn-side threading: `SubAgentTask` carries an
   optional `agent_type`; `SubAgentSpec` carries the resolved `agent_name`
   and `persona`; `SpawnRejectedUnknownAgentType` rejects an unmatched
   `agent_type`; and `SubAgentSpecsResolved` resolves mode/model with the
   agent definition as the middle precedence layer (task field → agent
   definition → mode default). A named agent never changes the tool
   registry — capability stays a pure function of mode.

## Mode rules (summary)

- **Explore parent** → top-level Explore registries do not expose
  `spawn_agents`. If an Explore-origin spawn request is nevertheless handled
  (for example from an older in-flight turn), Work mode is rejected and only
  Explore sub-agents are valid.
- **Work / Branch / Direct parent** → can spawn either mode; at most one
  Work sub-agent active at a time per parent (one-writer invariant), and
  per single `spawn_agents` call. Multiple Explore sub-agents in parallel
  are unconstrained beyond the hard cap of 10 tasks per call.

The mode-validation, one-writer, and cwd-scoping rules are normative in
`subagents.allium` §§1–4.

Every sub-agent mode inherits or accepts only a cwd that canonicalises to an
existing non-root directory. The root-floor guard is independent of write
capability because read-only search/listing tools rooted at the filesystem root
can consume unbounded resources.

## Defaulting (config)

| Sub-agent mode | Default model | Default `max_turns` |
|----------------|--------------|---------------------|
| Explore | Cheapest model in the parent's provider family (e.g. haiku for Anthropic) | 20 |
| Work | Parent's model (inherited) | 50 |

`DEFAULT_SUBAGENT_TIMEOUT = 20 minutes` is the wall-clock safety-net;
`max_turns` is the primary budget. An explicit `model` field on the task
is validated against the LLM registry and rejected if unknown. See
`subagents.allium`'s `SubAgentSpecsResolved` rule for the full resolution
sequence.

## Tool availability

The two sub-agent tool registries live in
`crates/phoenix-ide/src/tools.rs` (`ToolRegistry::for_subagent_explore` /
`for_subagent_work`). Both include `submit_result` / `submit_error`;
neither includes `spawn_agents`, `ask_user_question`, `skill`, or
`propose_task` (REQ-SA-002 + REQ-AUQ-006). The Work variant adds `patch`
on top of the Explore set.

MCP tools are wrapped at runtime via `ToolRegistryExecutor::with_mcp` —
sub-agents share the parent's MCP manager. An
Explore-search-restricted MCP subset is a documented deferred refinement
(see `executive.md`).

## Database

No schema changes for sub-agents. Existing fields suffice:

```sql
CREATE TABLE conversations (
    id TEXT PRIMARY KEY,
    parent_conversation_id TEXT,    -- Set for sub-agents
    user_initiated BOOLEAN NOT NULL, -- FALSE for sub-agents
    ...
    FOREIGN KEY (parent_conversation_id)
        REFERENCES conversations(id) ON DELETE CASCADE
);
```

`active_work_subagents` (the one-writer counter) is held on the
runtime executor and intentionally *not* persisted — sub-agents do
not survive server restart, so the counter resets to 0 on restart
by construction.

## Aggregated results format

When all sub-agents complete, the parent's LLM receives a structured
summary message persisted in place of the `spawn_agents` placeholder.
See `runtime/executor.rs::persist_sub_agent_results` for the exact
JSON shape; conceptually:

```json
{
  "sub_agent_results": [
    {
      "agent_id": "uuid-1",
      "task": "Review error handling from a security perspective",
      "outcome": { "success": { "result": "Found 3 issues: ..." } }
    },
    {
      "agent_id": "uuid-2",
      "task": "Review error handling from a performance perspective",
      "outcome": { "failure": { "error": "...", "error_kind": "sub_agent_error" } }
    }
  ]
}
```

## Example Use Cases

### Multi-Perspective Code Review

```
User: Review the authentication module for potential issues

Agent calls spawn_agents with:
{
  "tasks": [
    { "task": "Review src/auth/ from a security perspective. Look for vulnerabilities, credential handling issues, and attack vectors." },
    { "task": "Review src/auth/ from a maintainability perspective. Assess code clarity, test coverage, and documentation." },
    { "task": "Review src/auth/ from a performance perspective. Identify bottlenecks, unnecessary allocations, or N+1 patterns." }
  ]
}

Three sub-agents analyze the same code with different lenses; parent
aggregates findings into comprehensive review.
```

### Codebase Exploration

```
User: I'm new to this project. Help me understand the architecture.

Agent calls spawn_agents with:
{
  "tasks": [
    { "task": "Explore the database layer. Document the schema, key queries, and data access patterns." },
    { "task": "Explore the API layer. Document the endpoints, request/response formats, and middleware." },
    { "task": "Explore the core business logic. Document the main abstractions and how they interact." }
  ]
}

Sub-agents explore different areas in parallel; parent synthesizes into
architectural overview.
```

### Focused Deep-Dive (Single Sub-Agent)

```
User: How does error handling work in this codebase?

Agent calls spawn_agents with:
{
  "tasks": [
    { "task": "Thoroughly investigate error handling patterns in this codebase. Trace how errors propagate from tools through the state machine to the API. Document the error types, conversion points, and user-facing messages." }
  ]
}

Single sub-agent does focused research without polluting parent's context
with exploration details.
```

### Comparative Analysis

```
User: Should we use approach A or B for the new feature?

Agent calls spawn_agents with:
{
  "tasks": [
    { "task": "Analyze approach A: [description]. Evaluate pros, cons, implementation complexity, and how it fits with existing patterns in this codebase." },
    { "task": "Analyze approach B: [description]. Evaluate pros, cons, implementation complexity, and how it fits with existing patterns in this codebase." }
  ]
}

Sub-agents research independently without biasing each other; parent
makes informed recommendation based on both analyses.
```

### Persona-Based Review

```
User: Get feedback on this API design from different stakeholders

Agent calls spawn_agents with:
{
  "tasks": [
    { "task": "Review the API design as a frontend developer. Is it easy to consume? Are the response shapes convenient? Is error handling clear?" },
    { "task": "Review the API design as a DevOps engineer. Is it easy to monitor? Are there health checks? How's the logging?" },
    { "task": "Review the API design as a new team member. Is it well documented? Are the conventions consistent? Can you understand it without tribal knowledge?" }
  ]
}

Different perspectives surface different issues.
```
