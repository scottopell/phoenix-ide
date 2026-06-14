# DenyGate — deterministic-deny permission seam (Layer 0)

Lift the bash command-safety check out of the bash tool and up to the tool
dispatch chokepoint as a **tool-agnostic, correct-by-construction permission
seam**. This is the foundation layer; it ships the existing 3 bash rules
unchanged in behaviour but relocates and re-types them so future risk layers
(Tier A encoder, task 29001) bolt on behind the same seam without re-plumbing.

Spec: `specs/permissions/`.

## Problem

The only guardrail today is `bash_check::check`, called from inside
`bash/operations.rs::run_run`. Two structural problems:

1. **It lives inside one tool.** Every other tool (patch, browser, MCP, ...) has
   no checkpoint. A new tool re-implements its own checks or has none — the same
   drift the bash-internal check already represents.
2. **It's a call you must remember to make.** A future dispatch path (the way
   `spawn_agents` got special-cased above the normal path) can reach a tool
   without passing the check. Nothing in the types prevents an ungated call.

## Design — correct-by-construction token

The seam is a parse-don't-validate proof token. The gate is the **only**
constructor of the value the executor accepts for execution.

```rust
/// Proof that the deny gate cleared this call. Carries the validated payload
/// so a token for tool A cannot run tool B. No public constructor — the sole
/// mint is DenyGate::check. Test-only constructor behind #[cfg(test)].
pub struct CheckedToolCall { name: String, input: Value }

pub enum Denial { /* reason: String, rule id, ... */ }

impl DenyGate {
    pub fn check(&self, name: String, input: Value)
        -> Result<CheckedToolCall, Denial>;
}
```

`ToolExecutor::execute` changes signature to consume the proof:

```rust
// before
async fn execute(&self, name: &str, input: Value, ctx: ToolContext) -> Option<ToolOutput>;
// after
async fn execute(&self, call: CheckedToolCall, ctx: ToolContext) -> Option<ToolOutput>;
```

An ungated tool call becomes unrepresentable: `tool.run()` and the MCP
`mcp_tool.run()` are both past the `execute()` signature, so neither can be
reached without a `CheckedToolCall`, and the only mint is `DenyGate::check`.

## Placement — decision + proof split

- **Decision** runs synchronously in `dispatch_tool_execution`
  (`runtime/executor.rs`), on `&mut self`, BEFORE the `tokio::spawn` of the tool
  task. This is upstream of the registry-vs-MCP split (which lives inside
  `ToolExecutor::execute` at `runtime/traits.rs`), so a name+input gate here
  covers MCP tools too.
- On `Deny`: increment the executor-local denial counter, synthesize the denial
  outcome (`ToolOutput::error` → `tool_output_to_outcome` →
  `ToolExecOutcome::Completed`), forward through the existing `outcome_tx`, never
  spawn. The model receives a denial structurally identical to today's
  `command_safety_rejected`.
- On `Allow`: thread the returned `CheckedToolCall` into the spawn; `execute`
  consumes it.

The counter lives on the executor because the escalation thresholds are
conversation-loop state. The gate *logic* runs executor-side; the gate *proof*
is required at the `traits.rs` boundary. Both properties hold at once.

## Deny-and-continue + escalation

A denial returns to the model as a tool result (it already does, for bash). Add
executor-local counters:

- **Consecutive denials** — reset on any allowed/successful tool outcome.
- **Total denials** — persists for the conversation.
- Threshold (consecutive OR total exceeded) → stop driving the model, surface to
  the human. Exact thresholds are a design decision in `specs/permissions/`
  (Anthropic's reference: 3 consecutive OR 20 total).

`spawn_agents` stays special-cased above the gate — subagent delegation is
intent-aware (Tier B) territory, deliberately out of this intent-agnostic layer.

## Rule representation

Typed Rust registry keyed by tool name (generalize the current `bash_check`
shape). NOT declarative config — single-user self-hosted Phoenix does not need
the user/managed scoping whose main threat (shared/multi-tenant repos granting
themselves permissions) is weak here. Seed entries = the existing 3 bash rules
(blind `git add`, force push, dangerous `rm`), behaviour unchanged, including the
AST-based `brush_parser` matching and the `command_safety_rejected` error id /
`reason` shape.

## Scope

In:
- `DenyGate` + `CheckedToolCall` + `Denial` types; sole-mint invariant
  (private fields, `#[cfg(test)]` constructor).
- `ToolExecutor::execute` signature change + the 3-4 impls
  (`ToolRegistryExecutor`, the test mocks in `runtime/traits.rs` /
  `runtime/testing.rs` / `runtime/executor.rs`).
- Move bash deterministic rules into the registry; `bash_check` becomes a rule
  in the registry rather than a bash-internal call.
- Executor-side decision + denial counter + escalation threshold.
- `specs/permissions/` spEARS trio (+ Allium for the escalation lifecycle).
- Update `specs/bash` REQ-BASH-011 to defer the safety check to the permission
  seam (bash no longer owns it).

Out:
- Tier A encoder (task 29001) — bolts on as a second `DenyGate::check` stage.
- Tier B / overeagerness / transcript access / `ToolContext` changes.
- Declarative config rules.

## Acceptance

- An ungated tool call does not compile: `execute` consumes `CheckedToolCall`,
  whose only non-test constructor is `DenyGate::check`.
- The 3 existing bash rules deny identically (same `command_safety_rejected`
  error id + `reason`); existing `bash_check` tests pass relocated.
- A denied call never spawns a tool task; the model receives the denial via the
  existing outcome channel; the consecutive counter resets on the next allowed
  call.
- Escalation threshold stops the loop and surfaces to the human.
- MCP tools route through the gate (covered by the upstream placement).
- `specs/permissions/` parses (`allium check` zero errors) and the four spec
  views agree; `specs/bash` REQ-BASH-011 cross-references the seam.
