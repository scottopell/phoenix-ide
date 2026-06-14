# Permissions — Design

## Shape

The permission seam is a proof-token gate at the tool-dispatch chokepoint. It
splits cleanly into a **decision** (evaluate the pending call) and a **proof**
(the value that authorizes execution). The decision runs in the conversation
runtime where loop state lives; the proof is required at the executor trait
boundary where every tool — registry or MCP — converges.

```
LLM tool_use
  → dispatch_tool_execution            (runtime/executor.rs)   ── decision here
      DenyGate::check(name, input)
        ├─ Deny  → count, synthesize denial outcome, forward, DO NOT spawn
        └─ Allow → CheckedToolCall ──┐
                                     ↓ threaded into the spawned task
  → ToolExecutor::execute(CheckedToolCall, ctx)  (runtime/traits.rs) ── proof required
      ├─ registry tool.run(...)
      └─ live MCP mcp_tool.run(...)
```

## Correct-by-construction token

```rust
pub struct CheckedToolCall {
    name: String,
    input: Value,
}

pub enum Denial {
    // stable error id + human/model-readable reason; rule identity
}

pub struct DenyGate { /* typed rule registry, keyed by tool name */ }

impl DenyGate {
    pub fn check(&self, name: String, input: Value)
        -> Result<CheckedToolCall, Denial>;
}
```

Three properties make an ungated call unrepresentable:

1. `CheckedToolCall` has **private fields and no public constructor**. The sole
   non-test mint is `DenyGate::check`. A `#[cfg(test)]` constructor exists for
   mocks and cannot leak into production paths.
2. `ToolExecutor::execute` consumes a `CheckedToolCall` rather than a separate
   `(name, input)`. Both execution paths inside it — registry `tool.run` and the
   MCP fallback `mcp_tool.run` — are past that signature, so neither is reachable
   without a proof.
3. The proof **carries** the validated payload, and execution acts on the carried
   payload. A proof minted for tool A cannot be replayed to run tool B.

This is parse-don't-validate: `DenyGate::check` is the parser that turns a raw
`(name, input)` into the only type the executor will run.

## Placement: decision vs proof

The decision runs **synchronously** in `dispatch_tool_execution`, on the
executor's `&mut self`, before the tool task is spawned. Two reasons:

- **Upstream of the registry/MCP split.** The split lives inside
  `ToolExecutor::execute`; a `(name, input)` decision made before dispatch covers
  every tool class without a per-tool hook (REQ-PERM-003).
- **Loop-local counters.** The denial counters (REQ-PERM-006) are
  conversation-loop state. Deciding on `&mut self` lets a denial mutate them
  directly — no shared atomics, no channel back from a detached task.

On `Deny` the runtime never spawns the tool task. It synthesizes the denial as a
tool output, runs it through the same outcome conversion a real tool result uses,
and forwards it on the existing outcome channel. The model sees a denial shaped
exactly like a normal tool result (REQ-PERM-005).

On `Allow` the `CheckedToolCall` threads into the spawned task and is handed to
`ToolExecutor::execute`.

The gate *logic* is executor-side; the gate *proof* is required at the trait
boundary. Both the loop-local-counter property and the unrepresentable-bypass
property hold simultaneously — they are not in tension.

## Layer 0 rule registry

Layer 0 is a typed registry keyed by tool name. The bash entry retains the
existing AST approach: the command is parsed with `brush_parser` into a full
syntax tree and matched in every position (pipelines, `&&`/`||` chains, `;`
sequences, subshells, conditional bodies), with a leading `sudo` stripped before
matching. The matched patterns and the `command_safety_rejected` error id /
`reason` shape are unchanged from the prior bash-internal check — only their home
moves, from inside the bash tool to a registry entry behind the seam.

Rules are intent-agnostic: they read only `(name, input)` and environment trust,
never the transcript. This is the structural precondition that lets risk layers
stack behind Layer 0 without ever threading conversation context — and is the
same property that makes those layers injection-blind.

## Escalation lifecycle

Per-conversation counters track consecutive and total denials. Any allowed,
executed tool call resets the consecutive count; the total count only grows. When
consecutive reaches 3 or total reaches 20, the runtime stops driving the model
and surfaces to the human. The thresholds match the reference design's bounded-
autonomy posture: the consecutive trigger catches a model stuck retrying variants
of a blocked action; the total trigger catches slow accumulation across a long
session.

## Subagent delegation

The subagent-delegation path is special-cased ahead of the gate and does not pass
through Layer 0 (REQ-PERM-007). Delegation authorization is an intent-relative
judgment reserved for the intent-aware layer; Layer 0 stays purely intrinsic.

## Design Decisions

- **Typed registry, not declarative config.** Phoenix is single-user and
  self-hosted; the shared-repo self-granting threat that motivates user/managed
  config scoping does not apply. Rules change at development speed, so a compiled
  registry is type-safe and sufficient. Revisit only if multi-tenant hosting
  becomes a goal.
- **Decision executor-side, proof at the trait boundary.** Considered placing the
  whole gate inside `ToolExecutor::execute` for a single universal chokepoint.
  Rejected: the gate would then run inside the detached tool task, divorcing the
  denial counters from loop state. Splitting decision (executor) from proof
  requirement (trait) keeps counters loop-local while preserving the
  unrepresentable-bypass guarantee.
- **Risk layers stack behind Layer 0, never replace it.** Layer 0 is a hard,
  non-overridable floor. Soft risk layers (the on-device encoder) run only on
  calls Layer 0 allowed and can add denials, never remove a Layer 0 denial.

## Failure Modes

- **A new dispatch path reaches a tool ungated.** Prevented by construction:
  execution consumes `CheckedToolCall`, the only mint is `DenyGate::check`. A new
  caller cannot compile without going through the gate.
- **Proof/payload desync.** Prevented: the proof carries the payload; execution
  acts on the carried value.
- **Denial loop.** Bounded by the escalation thresholds (REQ-PERM-006).
- **Counter starvation across compaction.** Counters are runtime state on the
  conversation loop, not transcript-derived, so context compaction does not lose
  them.
