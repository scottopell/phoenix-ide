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
execution alike are unreachable through that boundary without it — an ungated
call does not compile. The guarantee is scoped to the runtime executor boundary
(the only path the runtime uses to execute an LLM tool call); the lower-level
`Tool::run` primitive is not sealed, but the registry exposes no ungated
`execute` shortcut. The decision runs in owned read-only preparation, before
ordinary start/channel/token/generation/context/metrics resources. An immutable
runtime/assistant/call origin guards acceptance; only its private cleared proof
reaches resource admission. Denial becomes a paired ordinary completion without
an invocation or outcome forwarder. A fixed class of reducer-intercepted calls (`spawn_agents`,
`propose_task`, `ask_user_question`, `submit_result`, `submit_error`) never reach
the gate and is gated by typed transitions instead. Layer 0 seeds with the three
existing bash rules (blind `git add`, force push, dangerous `rm`), behaviour and
`command_safety_rejected` wire shape unchanged, relocated from inside the bash
tool.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-PERM-001:** Single enforced seam | ✅ Delivered | `CheckedToolCall` sole mint = `DenyGate::check`; consumed by `ToolExecutor::execute`. Scoped to the executor boundary; raw `ToolRegistry::execute` shortcut removed |
| **REQ-PERM-002:** Proof carries payload | ✅ Delivered | Parse-don't-validate; proof holds the validated `(name, input)` |
| **REQ-PERM-003:** MCP coverage | ✅ Delivered | Gate runs in owned preparation upstream of registry/MCP execution |
| **REQ-PERM-004:** Layer 0 deterministic deny | Partially implemented | No-resource denial is implemented in task 75010. Focused checks prove unchanged generation, no start or metrics owner, no execution, and ordinary denial/continue behavior. Full static/default, isolated review and hosted qualification remain pending. |
| **REQ-PERM-005:** Denials as tool results | ✅ Delivered | Deny-and-continue via exact selected-call ordinary completion |
| **REQ-PERM-006:** Counting + escalation | ⬜ Planned (phase 2) | 3 consecutive OR 20 total → human; needs a net-new state-machine halt state |
| **REQ-PERM-007:** Typed internal exception | ✅ Delivered | The exact named class retains its own typed validation and never enters generic `ToolExecutor::execute`/arbitrary `Tool::run`. `spawn_agents` is runtime-owned admission; proposal/question/terminal submission are reducer-owned. The owner-neutral requirement corrects the implementation description without expanding the exception policy. |

**Progress:** 5 of 7 delivered; REQ-PERM-004 resource ordering is implemented with focused and owning coverage, awaiting full qualification. REQ-PERM-006 (counting + escalation) is planned.

## Scope Boundaries

- **In:** the seam, the CBC token, Layer 0 deterministic deny, deny-and-continue,
  escalation counters.
- **Out (separate work):** the on-device dangerous-action encoder (intent-agnostic
  risk layer behind Layer 0); intent-aware enforcement / overeagerness (Tier B,
  needs the transcript); declarative config rules.
