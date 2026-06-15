# Permissions — Requirements

## Overview

Phoenix gates every consequential tool call through a single **permission seam**
at the tool-dispatch chokepoint. The seam is layered: a deterministic deny layer
(hard guarantee) runs first, and consequential-action risk layers run behind it.
This document specifies the deterministic deny layer (Layer 0) and the
enforcement structure all layers share.

The governing property is correct-by-construction: a tool call that has not
passed the seam is structurally unrepresentable, not merely discouraged by
convention.

## Terminology

- **Seam** — the single point through which all tool calls pass before execution.
- **DenyGate** — the component that evaluates a pending call and either mints a
  proof of clearance or returns a denial.
- **CheckedToolCall** — the proof token. Carries the validated `(name, input)`.
  Its sole non-test constructor is `DenyGate::check`.
- **Denial** — a structured rejection carrying a human- and model-readable reason.
- **Layer 0 / deterministic deny** — a typed rule registry keyed by tool name,
  evaluated with no reference to conversation context.
- **Consequential action** — a tool call that can alter state, exfiltrate data,
  or cross a trust boundary. Read-only actions are not consequential.

## Requirements

### REQ-PERM-001: Single enforced seam

WHEN a tool call is dispatched for execution through the conversation runtime
THE SYSTEM SHALL evaluate it through DenyGate before the tool executes
AND SHALL make execution via the runtime tool-executor boundary
(`ToolExecutor::execute`) reachable only with a CheckedToolCall proof.

THE SYSTEM SHALL provide exactly one non-test constructor of CheckedToolCall:
`DenyGate::check`.

**Rationale:** A check that is a function you must remember to call can be
bypassed by a new dispatch path. Making the proof token the only accepted input
to `ToolExecutor::execute` converts "forgot to gate" from a review-time catch
into a compile-time error.

**Scope of the guarantee.** The proof binds the *runtime tool-executor boundary*
— the only path by which the conversation runtime executes an LLM-issued tool
call. `Tool::run` and `ToolRegistry::find_tool` are lower-level primitives the
runtime reaches solely through the gated `ToolExecutor`; the registry exposes no
name+input `execute` helper that would offer an ungated shortcut. The guarantee
is therefore "no LLM tool call executes without a proof," not "the `Tool::run`
primitive is universally sealed." Hardening the primitives further (e.g. making
`run` reachable only via a proof) is possible but out of scope for Layer 0.

### REQ-PERM-002: Proof carries the validated payload

WHEN DenyGate clears a call
THE SYSTEM SHALL return a CheckedToolCall carrying the same `(name, input)` that
was evaluated
AND tool execution SHALL act on the payload carried by the proof, not on a
separately supplied name/input.

**Rationale:** A bare "checked" boolean permits checking command A and running
command B. Binding proof and payload into one value (parse-don't-validate) makes
that desync unrepresentable.

### REQ-PERM-003: Coverage includes dynamically resolved tools

WHEN a tool call targets a registry tool OR a live-resolved MCP tool
THE SYSTEM SHALL evaluate it through DenyGate before execution.

**Rationale:** The seam sits upstream of the registry-vs-MCP resolution split, so
no tool class bypasses it. Coverage is structural, not a per-tool opt-in.

### REQ-PERM-004: Deterministic deny layer (Layer 0)

THE SYSTEM SHALL evaluate Layer 0 with no reference to conversation transcript,
user messages, or prior tool results — only the pending `(name, input)` and
environment trust.

THE SYSTEM SHALL represent Layer 0 rules as a typed registry keyed by tool name.

THE SYSTEM SHALL reject the following bash patterns (parsed with a shell syntax
parser, checked in any pipeline / compound / conditional position, with a leading
`sudo` stripped before matching):

- Blind git add: `git add -A`, `git add .`, `git add --all`, `git add *`
- Force push: `git push --force`, `git push -f` (allow `--force-with-lease`)
- Dangerous rm: `rm -rf` targeting `/`, `~`, `$HOME`, `.git`, `*`, `.*`

WHEN a Layer 0 rule matches
THE SYSTEM SHALL return a Denial with stable error id `command_safety_rejected`
and a `reason` describing the matched pattern
AND SHALL NOT execute the call (no tool task is spawned, no resources reserved).

**Rationale:** Layer 0 is a hard guarantee — intent-agnostic, deterministic, not
overridable by any later layer. It is the floor beneath the soft risk layers.

### REQ-PERM-005: Denials surface to the model as tool results

WHEN DenyGate returns a Denial
THE SYSTEM SHALL deliver it to the model through the same tool-result channel a
normal tool output uses
AND the model SHALL be able to attempt an alternative call.

**Rationale:** Deny-and-continue. A denial costs one retry plus a corrective
nudge, not a dead session — the property that makes a low false-positive rate
survivable on long tasks.

### REQ-PERM-006: Denial counting and escalation

THE SYSTEM SHALL maintain, per conversation, a count of consecutive denials and a
count of total denials.

WHEN any tool call is allowed and executes
THE SYSTEM SHALL reset the consecutive-denial count to zero.

WHEN the consecutive-denial count reaches 3 OR the total-denial count reaches 20
THE SYSTEM SHALL stop driving the model and surface the condition to the human.

**Rationale:** Bounded autonomy. A model that cannot find a permitted path should
escalate rather than loop. The consecutive trigger catches a stuck model; the
total trigger catches slow grinding across a long session.

### REQ-PERM-007: Reducer-intercepted tool calls are a structural exception

Some LLM tool calls are intercepted by the conversation state-machine reducer
and handled as typed state transitions, never becoming an "execute this tool"
effect and so never reaching the runtime tool-executor boundary the seam gates.
This class is: subagent delegation (`spawn_agents`), task proposal
(`propose_task`), user questioning (`ask_user_question`), and subagent terminal
submission (`submit_result` / `submit_error`).

THE SYSTEM SHALL treat these as a structural exception to REQ-PERM-001 — they are
not gate-cleared by Layer 0 — AND each SHALL carry its own enforcement contract
in the spec that owns it (the reducer validates the typed transition; e.g.
`propose_task` is validated against the task file, subagent submission against
sub-agent state).

**Rationale:** These calls do not invoke a tool's `run` to take an arbitrary
action; they drive a specific, typed transition the reducer already validates.
Layer 0's question — "is this action intrinsically dangerous?" — does not apply.
A future permission layer must not assume the seam covers them; their gate is the
typed transition, not Layer 0. (Delegation specifically is also intent-relative —
whether a delegated task is authorized is a Tier B judgment, not an intrinsic
one — which is a second reason it sits outside intent-agnostic Layer 0.)

## Non-Goals

- **Intent-aware enforcement (Tier B).** Judging an action against what the user
  authorized requires the transcript and is specified separately. Layer 0 and the
  intent-agnostic risk layers deliberately never read conversation context.
- **Declarative / config-scoped rules.** Rules are a typed Rust registry.
  Single-user self-hosted Phoenix does not face the shared-repo self-granting
  threat that motivates user/managed config scoping.
- **A security boundary at the tool layer.** The seam is enforcement of intended
  behaviour, not containment. Primary containment is git-worktree isolation and
  (on Linux) Landlock; see `specs/bash` and `specs/projects`.

## Related Specs

- `specs/bash` — the bash tool; its command-safety requirement defers to this
  seam. Landlock / worktree containment is the orthogonal security boundary.
- The on-device dangerous-action encoder (intent-agnostic risk layer) bolts on as
  a second DenyGate stage after Layer 0.
