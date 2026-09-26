# Sub-Agents

## User Story

As a developer using PhoenixIDE, I need the agent to delegate independent
tasks to parallel sub-agents so that complex operations complete faster and
the agent can synthesize multiple perspectives without exhausting its own
context window.

> Detailed behaviour (states, transitions, authority rules, qualified Work
> admission, and cwd scoping) lives in [`subagents.allium`](./subagents.allium)
> and [`bedrock.allium`](../bedrock/bedrock.allium).
>
> Named workers supply optional personas and execution preferences; see
> [`../agents/requirements.md`](../agents/requirements.md). A task can select a
> worker, a configured tier, or an exact model route. These choices do not grant
> authority.

## Requirements

### REQ-SA-001: Parallel Task Execution

WHEN LLM requests sub-agent spawn with one or more tasks
THE SYSTEM SHALL validate and admit the complete batch atomically before any child starts
AND SHALL create an independent conversation for each admitted task
AND execute admitted Explore children in parallel
AND execute admitted Work children according to the parent-model qualification policy in REQ-PROJ-008

IF any task in a requested batch is invalid or the complete batch is not admissible
THEN THE SYSTEM SHALL admit no child from that batch

WHEN spawning sub-agents
THE SYSTEM SHALL assign a mandatory time limit to each sub-agent
AND default execution authority to read-only if not specified (see REQ-PROJ-008)
AND enforce a max turn limit per sub-agent (read-only: 20, write-capable: 50, overridable)

WHEN more than 10 sub-agents are requested in a single spawn call
THE SYSTEM SHALL reject the call with an error

**Rationale:** Users benefit from parallel task execution for code review,
exploration, and divide-and-conquer problem solving. Spawning sub-agents
keeps the parent's context clean for synthesis. The default authority is
read-only (cheap, safe) unless the LLM explicitly requests write capability
against the parent's attached `WorkScope`.

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
AND persist terminal evidence before reporting the outcome
AND resolve the receiving parent from the child's durable parent-conversation identity

WHEN the parent accepts a terminal outcome for an admitted child
THE SYSTEM SHALL treat repeated delivery of that child's terminal outcome as idempotent success
AND SHALL NOT append a second result or resume the parent more than once

**Rationale:** Explicit result submission provides clean completion
semantics. The terminal-tool-must-be-alone constraint prevents ambiguity
about whether other tools in the same response should execute.

---

### REQ-SA-004: Parent Fan-In

WHEN sub-agents are admitted
THE SYSTEM SHALL track each admitted child by durable identity as exactly one of pending or parent-accepted

WHEN an admitted child's terminal outcome is accepted
THE SYSTEM SHALL remove that exact child from pending and add exactly one completed result

WHEN every admitted child has a parent-accepted terminal outcome
THE SYSTEM SHALL aggregate all results
AND return them to the parent conversation for the LLM to process
AND SHALL NOT settle or resume the parent while any admitted child remains pending

WHEN a sub-agent result arrives before the parent is ready to receive it
THE SYSTEM SHALL buffer the result without losing it

**Rationale:** Users need reliable aggregation regardless of completion
order. The parent LLM receives all outcomes (successes and failures) to
make informed decisions.

---

### REQ-SA-005: Cancellation Propagation

WHEN user cancels the parent conversation while sub-agents are admitted
THE SYSTEM SHALL durably request cancellation for every pending child
AND wait for every admitted child to reach a parent-accepted terminal outcome before ordinary settlement

WHEN cancellation is requested before a child's initial work starts
THE SYSTEM SHALL prevent that initial work from starting
AND produce one terminal cancellation outcome for parent fan-in

WHEN cancellation races child materialization or initial start
THE SYSTEM SHALL atomically choose either cancellation-before-start or one initial start followed by cancellation
AND SHALL NOT perform initial work more than once

WHEN a child runtime must be created or recovered
THE SYSTEM SHALL join concurrent creation attempts for that child identity
AND materialize at most one live runtime and dispatch initial work at most once

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

THE SYSTEM SHALL accept one optional execution selector per task: a configured
tier or an explicit model and connection with optional effort
AND SHALL allow the selector independently of an optional named worker
AND SHALL NOT accept simultaneous tier and explicit-model selections

WHEN execution is omitted
THE SYSTEM SHALL use the named worker's ordered candidates if declared,
otherwise inherit the parent's model, connection, and effort as one choice

WHEN explicit model execution omits effort
THE SYSTEM SHALL use the selected model's native default, without inheriting
worker, tier, or parent effort

WHEN execution explicitly names a model and connection
THE SYSTEM SHALL use that exact route or reject it, without fallback

THE SYSTEM SHALL expose only usable model routes and eligible tiers, with
explicit effort choices constrained to known supported values
AND SHALL validate the whole batch before creating any child

WHEN a child runtime is recreated during its run
THE SYSTEM SHALL restore its resolved model, effort, connection, and persona
without reapplying configuration preferences

**Rationale:** Cost and capability are independent of permissions. Ordered
preferences provide portable defaults; explicit choices remain exact. Logical
connection identity binds a configured backend slot, not an account or endpoint
across operator reconfiguration. Runtime recreation does not imply survival
across server restart.

**Dependencies:** REQ-AG-005, REQ-AG-008, REQ-AG-011, REQ-AG-012

---

### REQ-PROJ-008: Sub-Agent Capabilities Inherit the Parent Workspace Authority

WHEN a Git-backed parent conversation spawns a sub-agent with write authority requested
THE SYSTEM SHALL configure the sub-agent's working directory as the parent's worktree
AND grant write access to that same worktree
AND place the parent conversation in AwaitingSubAgentResult state for the duration
AND SHALL NOT provision a fresh detached-default-branch disposable worktree for that sub-agent
AND SHALL resolve and carry the parent's exact durable `WorkScope` identity in the spawned sub-agent specification
AND SHALL attach the sub-agent to that exact `WorkScope` identity rather than inferring attachment from filesystem path equality

WHEN deciding whether multiple Work children may be pending for one parent
THE SYSTEM SHALL qualify only the parent's actual resolved model identifier
AND SHALL qualify exactly `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-6-astra`, and `gpt-6-sol`
AND SHALL NOT infer qualification from provider, model family, version ordering, reasoning effort, service tier, configuration tier, named worker, child model, or child persona
AND SHALL treat `gpt-5.6-luna`, `gpt-6-luna`, every unknown or newly introduced identifier, every custom route, and every otherwise unlisted parent model as unqualified

WHEN a qualified parent requests a valid bounded batch
THE SYSTEM SHALL allow multiple Work tasks in that batch and multiple pending Work children across calls

WHEN an unqualified parent requests Work tasks
THE SYSTEM SHALL admit at most one Work child while no other admitted Work child remains pending
AND SHALL reject a batch containing multiple Work tasks
AND SHALL reject a new Work admission while an earlier Work child remains pending

WHEN the parent's resolved model changes
THE SYSTEM SHALL preserve every child and result admitted before the change
AND SHALL use the newly resolved parent model only for subsequent admission decisions

WHEN qualified parallel Work children share a `WorkScope`
THE SYSTEM SHALL identify them as trusted collaborators in parent and child instructions
AND SHALL instruct the parent to partition assignments and integrate results
AND SHALL instruct each child to inspect and preserve unrelated edits and report overlap, conflicts, or uncertainty
AND SHALL NOT promise structural prevention of overlapping writes

WHEN a Git-backed parent conversation spawns a sub-agent with read-only authority requested
THE SYSTEM SHALL configure the sub-agent's working directory as the parent's worktree
AND grant read-only authority there
AND allow multiple read-only sub-agents in parallel
AND SHALL NOT provision a fresh detached-default-branch disposable worktree for that sub-agent
AND SHALL resolve and carry the parent's exact durable `WorkScope` identity in the spawned sub-agent specification
AND SHALL attach the sub-agent to that exact `WorkScope` identity rather than inferring attachment from filesystem path equality

WHEN a planning/read-only conversation spawns sub-agents
THE SYSTEM SHALL configure those sub-agents with read-only authority

**Rationale:** Execution authority remains independent from orchestration qualification. Explicitly qualified parents may coordinate trusted writers in one owned environment; every other parent fails closed to sequential Work admission. Exact durable `WorkScope` attachment keeps shared authority structural, while collaborator instructions make integration and conflict reporting explicit without claiming arbitrary-write atomicity.

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

WHEN any sub-agent receives its turn-limit grace request
THE SYSTEM SHALL expose only `submit_result` and `submit_error` as callable tools
AND SHALL preserve completed ordinary-tool history as context for the terminal answer

WHEN a retryable provider failure occurs during the grace request
THE SYSTEM SHALL retain the terminal-only tool surface on the retry

**Rationale:** A prose instruction cannot make an advertised ordinary tool
uncallable. The request capability and reducer admission rule must agree so a
model cannot spend its only grace response on an action Phoenix must reject.

---

### REQ-SA-011: Spawn Override Defaults and Path Base

WHEN `cwd` is omitted, blank, or whitespace-only in a sub-agent task
THE SYSTEM SHALL inherit the parent working directory

WHEN execution is omitted
THE SYSTEM SHALL use the default defined by REQ-SA-007

WHEN a task supplies a relative `cwd` override
THE SYSTEM SHALL resolve it from the parent conversation's working directory
AND SHALL validate the resolved path with the same existence, non-root, symlink,
and Work-worktree containment rules as an absolute override

WHEN a task supplies an execution selector
THE SYSTEM SHALL validate it against the same resolved catalog snapshot used to
advertise choices for that request
AND SHALL reject an invalid selection before spawning any task in the batch

**Rationale:** Defaults should be the easiest valid representation. Empty path
placeholders and server-process-relative paths must not move a child outside the
parent's working context. A callable selection must not depend on hidden defaults
that differ between schema construction and admission.
