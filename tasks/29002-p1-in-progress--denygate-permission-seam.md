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

### Phase 1 — CBC seam + Layer 0 relocation (DELIVERED)
- `DenyGate` + `CheckedToolCall` + `Denial` types in `runtime/deny_gate.rs`;
  sole-mint invariant (private fields, `#[cfg(test)]` constructor).
- `ToolExecutor::execute` signature change (`CheckedToolCall`) + all impls
  (`ToolRegistryExecutor`, `Arc<T>` blanket, the test mocks in
  `runtime/testing.rs` / `runtime/executor.rs`).
- Gate decision in `dispatch_tool_execution` before the spawn; deny → synthesize
  `ToolOutput::error` via the existing `outcome_tx` (deny-and-continue works).
- bash deterministic check relocated: gate's bash rule calls
  `phoenix_tools::bash_check::check`; the inline call + `BashError`/
  `BashErrorResponse` construction removed from the bash tool.
- `specs/permissions/` spEARS trio + Allium; `specs/bash` REQ-BASH-011 relocated.

### Phase 2 — counters + escalation (DEFERRED, follow-up)
- Per-conversation consecutive/total denial counters on the runtime.
- Escalation threshold (3 consecutive OR 20 total) → **stop driving the model,
  surface to human**. Needs a net-new state-machine state (no
  awaiting-human/escalated state exists today) — that design is why it is split
  out. REQ-PERM-006 in `specs/permissions/` is the contract; until phase 2 the
  executive status table shows it Planned.

### F1 hardening (review follow-up)
- Phase 1 removed the dead `phoenix_tools::ToolRegistry::execute` raw name+input
  shortcut so the runtime reaches `Tool::run` only via the gated `ToolExecutor`.
- The `Tool::run` primitive itself is still callable in principle (it is a public
  trait method). Fully sealing it behind the proof (so no caller can reach `run`
  without a `CheckedToolCall`) is possible future hardening; the spec scopes the
  guarantee to the executor boundary rather than overclaiming a universal seal.

### Phase 3 — wire-type unification (DEFERRED, cleanup)
- Phase 1 keeps `BashErrorResponse::CommandSafetyRejected` as the documented
  wire contract (now unconstructed in Rust; the gate's `Denial::into_tool_output`
  emits the byte-identical shape) to avoid UI/codegen churn. Unify by moving
  `command_safety_rejected` into a permission-layer wire type + one valibot edit.

Out (all phases):
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
