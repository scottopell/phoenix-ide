# Sub-Agents

## User Story

As a developer using PhoenixIDE, I need the agent to delegate independent
tasks to parallel sub-agents so that complex operations complete faster and
the agent can synthesize multiple perspectives without exhausting its own
context window.

> Detailed behaviour (states, transitions, invariants, mode rules,
> one-writer constraint, cwd-scoping) lives in
> [`subagents.allium`](./subagents.allium) and
> [`bedrock.allium`](../bedrock/bedrock.allium). This file records user
> need, rationale, and per-requirement status only.
>
> Named agents ([`../agents/`](../agents/requirements.md)) extend the spawn
> path: a task may carry an `agent_type` that supplies the spawned sub-agent's
> persona and its default model and mode. The unknown-`agent_type` rejection
> and the resolution precedence (task field → agent definition → mode default)
> are normative in [`subagents.allium`](./subagents.allium); persona discovery
> and composition are normative in [`agents.allium`](../agents/agents.allium).

## Requirements

### REQ-SA-001: Parallel Task Execution

WHEN LLM requests sub-agent spawn with one or more tasks
THE SYSTEM SHALL create an independent conversation for each task
AND execute all sub-agent conversations in parallel

WHEN spawning sub-agents
THE SYSTEM SHALL assign a mandatory time limit to each sub-agent
AND default mode to Explore if not specified (see REQ-PROJ-008)
AND enforce a max turn limit per sub-agent (Explore: 20, Work: 50, overridable)

WHEN more than 10 sub-agents are requested in a single spawn call
THE SYSTEM SHALL reject the call with an error

**Rationale:** Users benefit from parallel task execution for code review,
exploration, and divide-and-conquer problem solving. Spawning sub-agents
keeps the parent's context clean for synthesis. Mode defaults to Explore
(cheap, read-only) to minimize cost unless the LLM explicitly opts into
Work mode.

**Dependencies:** REQ-BED-008

---

### REQ-SA-002: Sub-Agent Isolation

WHEN sub-agent is executing
THE SYSTEM SHALL maintain completely independent state from parent conversation
AND prevent sub-agents from spawning their own sub-agents

WHEN sub-agent conversation exists
THE SYSTEM SHALL track it as non-user-initiated
AND exclude it from normal conversation listings

**Rationale:** Users need isolation guarantees to prevent cascading
failures, resource exhaustion, and unbounded recursion.

**Dependencies:** REQ-BED-009

---

### REQ-SA-003: Result Submission

WHEN sub-agent completes its task
THE SYSTEM SHALL require it to call a dedicated result submission tool
AND the result submission tool SHALL be the only tool in that LLM response

WHEN sub-agent encounters an unrecoverable error
THE SYSTEM SHALL provide a dedicated error submission tool
AND the error submission tool SHALL be the only tool in that LLM response

WHEN sub-agent submits a result or error
THE SYSTEM SHALL transition the sub-agent to a terminal state
AND report the outcome to the parent conversation

**Rationale:** Explicit result submission provides clean completion
semantics. The terminal-tool-must-be-alone constraint prevents ambiguity
about whether other tools in the same response should execute.

---

### REQ-SA-004: Parent Fan-In

WHEN sub-agents are running
THE SYSTEM SHALL track pending and completed sub-agent counts

WHEN all sub-agents have submitted results (success or failure)
THE SYSTEM SHALL aggregate all results
AND return them to the parent conversation for the LLM to process

WHEN a sub-agent result arrives before the parent is ready to receive it
THE SYSTEM SHALL buffer the result without losing it

**Rationale:** Users need reliable aggregation regardless of completion
order. The parent LLM receives all outcomes (successes and failures) to
make informed decisions.

---

### REQ-SA-005: Cancellation Propagation

WHEN user cancels the parent conversation while sub-agents are running
THE SYSTEM SHALL propagate cancellation to all pending sub-agents
AND wait for all sub-agents to acknowledge cancellation before returning to idle

WHEN sub-agent receives cancellation
THE SYSTEM SHALL terminate immediately regardless of current operation

**Rationale:** Cancellation must be comprehensive. Orphaned sub-agents
consuming resources after the parent is cancelled would confuse users and
waste compute.

---

### REQ-SA-006: Timeout Enforcement

WHEN sub-agent exceeds its time limit without submitting a result
THE SYSTEM SHALL terminate the sub-agent immediately
AND report timeout failure to the parent conversation

WHEN sub-agent timeout fires
THE SYSTEM SHALL NOT wait for the sub-agent to finish its current operation

**Rationale:** Without enforced time limits, a stuck or slow sub-agent
holds the parent conversation indefinitely. Users need assurance that
sub-agent work completes or fails within a bounded time.

**Dependencies:** REQ-BED-026

---

### REQ-SA-007: Model Selection

**Superseded by REQ-PROJ-008 (sub-agent modes).** The tier concept
(`fast`/`capable`) is replaced by mode-based defaults with optional
explicit model override:

- Explore mode defaults to the cheapest available model for the parent's
  provider family.
- Work mode inherits the parent's model.
- The `model` field on the task spec allows explicit override with any
  registry model id; unknown ids are rejected at spawn time.

**Rationale:** Mode-based defaults cover the same cost/capability
trade-off that tiers addressed, while the explicit model override handles
edge cases. Two layers of indirection (mode defaults + tier resolution)
added complexity without benefit.

---

### REQ-SA-008: Context Injection via Read-First Files

WHEN a sub-agent spawn spec includes a list of file paths in `read_first`
THE SYSTEM SHALL read each file at spawn time
AND inject the file contents into the sub-agent's system prompt before the task

WHEN a read_first file does not exist or cannot be read
THE SYSTEM SHALL reject the sub-agent spawn with an error listing the missing file

THE SYSTEM SHALL accept only exact file paths in read_first (no glob patterns)

**Rationale:** Effective sub-agent prompts need focused context — which
spec files to consult, which source files are relevant. Injecting files
into the system prompt ensures the sub-agent sees them before its first
LLM call, without spending a tool call to read them. Exact paths only
keeps context size predictable and prevents accidental injection of large
directory trees.

---

### REQ-SA-009: Durable Wake Handle Identity for Sub-Agent Terminals

WHEN a sub-agent is spawned
THE SYSTEM SHALL durably bind the child conversation / agent id to the wake-plane
resource identity, durable terminal-evidence source, and wake terminal-payload
mapping before any later engine selection, resume, or restart-time observation uses
that handle

THE SYSTEM SHALL expose that stable terminal-wait handle identified by the child
conversation / agent id

WHEN that handle is watched by a wake contract
THE SYSTEM SHALL report fired terminal outcomes for every durable child terminal
cause admitted by bedrock, including successful `submit_result`, `submit_error`,
wall-clock timeout, independently observed child cancellation, turn-limit
hard-stop fallback, implicit text completion, non-retryable runtime failure, and
context exhaustion, and SHALL
resolve missing child handles through the wake contract's `Forgotten` cause

THE SYSTEM SHALL persist the sub-agent terminal-cause discriminator required to
distinguish those outcomes durably; coarse success/failure state alone SHALL NOT
be the source for wake terminal payload reconstruction

WHEN Phoenix restarts while a sub-agent wake contract is pending
THE SYSTEM SHALL deliver the child conversation's persisted terminal state and its
durable terminal cause when that cause occurred before the contract deadline,
expire the wake contract when the child has durable terminal state only after the
contract deadline, and otherwise treat the sub-agent handle as forgotten because
active sub-agent runtimes do not survive restart

Existing `spawn_agents` fan-in SHALL remain compatibility sugar. The runtime MAY
lower that fan-in onto wake contracts internally. Explicit `wait_until` for
sub-agent handles SHALL be usable only when a parent already has a stable child id
from another surface; adding a non-blocking `spawn_agents` mode is out of scope
for v1.

THE sub-agent wake handle SHALL NOT be keyed by the parent's WorkScope and SHALL
NOT imply parent-to-child continuation or automatic budget extension

**Rationale:** Wake contracts need a stable way to reference sub-agent terminal
completion without embedding blocking fan-in semantics into every parent state.
The child conversation / agent id is already the durable sub-agent identity; the
wake plane reuses it rather than inventing a parallel handle namespace.

---

### REQ-SA-010: Turn-Limit Grace Prompt Integrity

WHEN a Work sub-agent reaches its turn limit and receives its grace turn
THE SYSTEM SHALL instruct it not to report incomplete implementation as successful
completion

WHEN the assigned Work task required code changes and the sub-agent has not made
them
THE SYSTEM SHALL instruct it to call `submit_error` while preserving useful
analysis, plan details, blockers, and partial progress for the parent

WHEN an Explore sub-agent reaches its turn limit
THE SYSTEM MAY continue to use analysis-oriented grace guidance because Explore
work commonly completes by reporting findings rather than edits

**Rationale:** The grace turn exists to force a terminal answer, not to relabel
unfinished Work as success. Parent synthesis is safer when incomplete
implementation is structurally visible in the terminal payload.
