# Design wake-plane async waits for process and sub-agent handles

## Problem

Phoenix currently has several spawn-and-wait flows that either block the parent conversation or require polling:

- bash/tmux waits consume repeated LLM turns when the process has not finished within the synchronous wait window;
- sub-agent fan-in is hardcoded as a blocking parent state rather than a general async handle wait;
- a parent cannot spawn work, return to Idle, and be woken when the watched handle reaches a terminal state.

The motivating spawn-agent incident also exposed a smaller prompt issue: when a sub-agent hits its turn budget, the grace-turn prompt can encourage a Work sub-agent to submit useful analysis as success even when implementation is incomplete. That prompt fix is useful but separate. The main design task is the wake plane for concrete async terminal waits, not a general conversation-actor framework.

## Concrete v1 use cases

### 1. Async process wait

A conversation starts or references a bash/tmux handle and wants to be notified when it reaches terminal state without spending LLM turns polling.

Expected behavior:

1. The LLM registers a wait on a handle, e.g. `wait_until({ handle: { kind: "bash", id }, condition: "terminal" })`.
2. The conversation returns to Idle and remains user-interruptible.
3. The wake router observes handle terminal state, expiry, cancellation, or forgotten handle state.
4. Phoenix appends a synthetic result to the conversation and triggers the next LLM turn.
5. The delivered payload is equivalent to the synchronous wait payload where applicable.

### 2. Async sub-agent terminal wait

A parent conversation spawns or references a sub-agent handle and wants to be woken when that child reaches terminal state, instead of relying on bespoke blocking fan-in semantics for every async delegation shape.

Expected behavior:

1. The parent has a stable sub-agent handle, keyed by the child conversation / agent id.
2. The parent registers a terminal wait on that handle.
3. The parent conversation returns to Idle and remains user-interruptible.
4. When the child reaches `submit_result`, `submit_error`, wall-clock timeout, turn-limit terminal behavior, cancellation, or forgotten state, the wake router delivers the terminal result to the parent.
5. The current sub-agent contract remains largely unchanged: sub-agents are still delegated jobs with terminal results. V1 does not add parent-to-child continuation.

## Proposed scope

### 1. Refine the wake-contracts spec around the two concrete v1 cases

Update or draft specs so v1 is explicitly driven by:

- bash/tmux terminal handle waits;
- sub-agent terminal handle waits.

Avoid designing a generic conversation-actor/request-reply system in this task. Future actor messaging can build on the same wake-plane infrastructure if justified by concrete use cases.

### 2. Define handle identity and lifecycle rules

Specify exactly how each v1 handle kind is addressed and when it becomes terminal/forgotten:

- bash handle identity and WorkScope inheritance behavior;
- tmux handle identity and WorkScope/conversation lifecycle behavior;
- sub-agent handle identity as child conversation / agent id, not WorkScope-keyed;
- behavior on Phoenix restart, hard-delete, parent cancellation, and child cancellation.

### 3. Define wake registration and delivery shape

Specify the LLM-facing and internal shape for v1:

- `wait_until` input shape with tagged handle discriminator;
- mandatory expiration defaults/caps;
- persisted contract fields;
- router polling/resync behavior;
- synthetic delivery into the conversation message log;
- SSE/status updates;
- user cancellation of pending waits.

### 4. Define sub-agent terminal payload delivery

Specify what a sub-agent terminal wake delivers to the parent:

- successful `submit_result`;
- `submit_error` with error kind;
- wall-clock timeout;
- cancellation;
- turn-limit hard-stop fallback if it occurs;
- forgotten child handle.

Do not add continuation or `NeedMoreBudget` request/reply semantics in v1.

### 5. Identify implementation sequence

Produce a shippable sequence, likely:

1. **Prompt PR:** improve the existing sub-agent grace-turn prompt so incomplete Work implementation is not presented as success.
2. **Spec PR:** tighten `specs/wake-contracts/` and `specs/subagents/` around process and sub-agent terminal waits.
3. **Wake runtime PR:** implement persisted wake contracts and router for bash/tmux terminal handles.
4. **Sub-agent wake PR:** expose/register sub-agent terminal handles through the same wake plane.
5. **Follow-up evaluation:** decide whether blocking `AwaitingSubAgents` fan-in remains as compatibility sugar or can be lowered onto wake contracts.

## Minor standalone fix: sub-agent grace-turn prompt

The current grace-turn prompt should be improved independently:

- Work sub-agents should be told: if the assigned task required code changes and none were made, do not call `submit_result` as if complete.
- They should call `submit_error` or explicitly report incomplete implementation, including useful analysis/plan details for the parent.
- Explore sub-agents can keep analysis-oriented guidance because their expected output is often findings rather than edits.

This is not the main wake-plane design, but it should be small enough to land separately.

## Non-goals

- Do not design or implement a general conversation-actor framework in v1.
- Do not implement `continue_subagent`, `NeedMoreBudget`, or parent-to-child request/reply semantics in v1.
- Do not make runtime heuristics auto-extend sub-agents without parent involvement.
- Do not introduce prompt-only `spawn_agents` knobs such as `execution_bias` as a substitute for wake-plane delivery.
- Do not support arbitrary child clarification questions in v1.

## Discussion points

- Whether `spawn_agents` should immediately return sub-agent handles for explicit `wait_until`, or whether existing blocking fan-in should be internally represented as wake contracts first.
- Exact synthetic message/tool-result shape for sub-agent terminal wakes.
- Whether wake delivery should trigger an LLM turn automatically in all cases or only when registered by an LLM tool call.
- How user messages interact with pending wake contracts while the conversation is Idle.
- Whether blocking `AwaitingSubAgents` should remain indefinitely for backward compatibility.
