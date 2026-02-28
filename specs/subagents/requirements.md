# Sub-Agents

## User Story

As a developer using PhoenixIDE, I need the agent to delegate independent tasks to parallel sub-agents so that complex operations complete faster and the agent can synthesize multiple perspectives without exhausting its own context window.

## Requirements

### REQ-SA-001: Parallel Task Execution

WHEN LLM requests sub-agent spawn with one or more tasks
THE SYSTEM SHALL create an independent conversation for each task
AND execute all sub-agent conversations in parallel

WHEN spawning sub-agents
THE SYSTEM SHALL assign a mandatory time limit to each sub-agent

**Rationale:** Users benefit from parallel task execution for code review, exploration, and divide-and-conquer problem solving. Spawning sub-agents keeps the parent's context clean for synthesis.

**Dependencies:** REQ-BED-008

---

### REQ-SA-002: Sub-Agent Isolation

WHEN sub-agent is executing
THE SYSTEM SHALL maintain completely independent state from parent conversation
AND prevent sub-agents from spawning their own sub-agents

WHEN sub-agent conversation exists
THE SYSTEM SHALL track it as non-user-initiated
AND exclude it from normal conversation listings

**Rationale:** Users need isolation guarantees to prevent cascading failures, resource exhaustion, and unbounded recursion.

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

**Rationale:** Explicit result submission provides clean completion semantics. The terminal-tool-must-be-alone constraint prevents ambiguity about whether other tools in the same response should execute.

---

### REQ-SA-004: Parent Fan-In

WHEN sub-agents are running
THE SYSTEM SHALL track pending and completed sub-agent counts

WHEN all sub-agents have submitted results (success or failure)
THE SYSTEM SHALL aggregate all results
AND return them to the parent conversation for the LLM to process

WHEN a sub-agent result arrives before the parent is ready to receive it
THE SYSTEM SHALL buffer the result without losing it

**Rationale:** Users need reliable aggregation regardless of completion order. The parent LLM receives all outcomes (successes and failures) to make informed decisions.

---

### REQ-SA-005: Cancellation Propagation

WHEN user cancels the parent conversation while sub-agents are running
THE SYSTEM SHALL propagate cancellation to all pending sub-agents
AND wait for all sub-agents to acknowledge cancellation before returning to idle

WHEN sub-agent receives cancellation
THE SYSTEM SHALL terminate immediately regardless of current operation

**Rationale:** Cancellation must be comprehensive. Orphaned sub-agents consuming resources after the parent is cancelled would confuse users and waste compute.

---

### REQ-SA-006: Timeout Enforcement

WHEN sub-agent exceeds its time limit without submitting a result
THE SYSTEM SHALL terminate the sub-agent immediately
AND report timeout failure to the parent conversation

WHEN sub-agent timeout fires
THE SYSTEM SHALL NOT wait for the sub-agent to finish its current operation

**Rationale:** Without enforced time limits, a stuck or slow sub-agent holds the parent conversation indefinitely. Users need assurance that sub-agent work completes or fails within a bounded time.

**Dependencies:** REQ-BED-026
