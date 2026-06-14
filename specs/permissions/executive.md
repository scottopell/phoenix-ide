# Permissions — Executive Summary

## Requirements Summary

Phoenix gates every consequential tool call through one enforced permission seam.
A deterministic deny layer (Layer 0) — intent-agnostic, typed rules keyed by tool
name — runs first as a hard floor; consequential-action risk layers stack behind
it. Denials return to the model as ordinary tool results (deny-and-continue), and
per-conversation denial counters escalate to the human when the model cannot find
a permitted path. Subagent delegation is deliberately excluded as intent-relative.

## Technical Summary

The seam is a parse-don't-validate proof token. `DenyGate::check(name, input)`
returns either a `Denial` or a `CheckedToolCall` whose sole non-test constructor
it is. `ToolExecutor::execute` consumes the proof, so registry and live-MCP tool
execution alike are unreachable without it — an ungated call does not compile.
The decision runs synchronously in `dispatch_tool_execution` (executor-side,
before the tool task spawns) so denial counters stay loop-local; the proof is
required at the executor trait boundary so coverage is universal. Layer 0 seeds
with the three existing bash rules (blind `git add`, force push, dangerous `rm`),
behaviour and `command_safety_rejected` wire shape unchanged, relocated from
inside the bash tool.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-PERM-001:** Single enforced seam | ⬜ Planned | `CheckedToolCall` sole mint = `DenyGate::check` |
| **REQ-PERM-002:** Proof carries payload | ⬜ Planned | Parse-don't-validate; no proof/payload desync |
| **REQ-PERM-003:** MCP coverage | ⬜ Planned | Gate upstream of registry/MCP split |
| **REQ-PERM-004:** Layer 0 deterministic deny | ⬜ Planned | Relocate 3 bash rules to typed registry |
| **REQ-PERM-005:** Denials as tool results | ⬜ Planned | Deny-and-continue via existing outcome channel |
| **REQ-PERM-006:** Counting + escalation | ⬜ Planned | 3 consecutive OR 20 total → human |
| **REQ-PERM-007:** Delegation out of Layer 0 | ⬜ Planned | `spawn_agents` special-cased above gate |

**Progress:** 0 of 7 implemented (specification stage).

## Scope Boundaries

- **In:** the seam, the CBC token, Layer 0 deterministic deny, deny-and-continue,
  escalation counters.
- **Out (separate work):** the on-device dangerous-action encoder (intent-agnostic
  risk layer behind Layer 0); intent-aware enforcement / overeagerness (Tier B,
  needs the transcript); declarative config rules.
