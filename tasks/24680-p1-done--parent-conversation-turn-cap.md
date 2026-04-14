---
created: 2026-04-14
priority: p1
status: done
artifact: src/runtime/executor.rs
---

# Parent conversations have no tool_use iteration cap

## Resolution

Added a separate `parent_tool_cycle_count` counter on `ConversationRuntime`
(distinct from sub-agent `llm_turn_count`, which has different semantics —
sub-agents have a hard lifetime cap, parents have a per-turn cap that
resets on every user message).

- New constant `DEFAULT_PARENT_TOOL_CYCLE_CAP = 1000`. Deliberately
  high: this is a backup safety-net, not a budget. A well-behaved
  agent should stay far below it; hitting the cap means something is
  stuck or looping.
- New field `parent_tool_cycle_cap: u32` on `ConversationRuntime`, sourced
  from `PHOENIX_PARENT_TOOL_CYCLE_CAP` at construction (set to `0` to
  disable). Test override via `with_parent_tool_cycle_cap(cap)`.
- Reset in `process_event` when the incoming event is `Event::UserMessage`
- Increment + check in `Effect::RequestLlm` for non-sub-agent runtimes
- On cap hit: `halt_parent_cycle_cap` persists a system message
  explaining what happened and dispatches `Event::UserCancel` so the
  state machine transitions cleanly back to `Idle` via the existing
  abort path. The user can then send a follow-up message — the counter
  resets on that next turn.

Regression test `test_parent_tool_cycle_cap_halts_runaway_loop` proves
that with `cap=3`, a mock LLM that always emits a `bash` tool_use stops
the runtime within a bounded number of rows (vs. the ~800 rows the bug
was producing), and that a system message containing "Tool-use iteration
limit" is persisted for the user.

## Problem

Sub-agents enforce `max_turns` (REQ-PROJ-008): Explore=20, Work=50,
with a grace turn to call `submit_result`. Parent / top-level
conversations do not — `context.max_turns` is `0`, and the check at
`src/runtime/executor.rs:795` is gated on `> 0`, so the guard is
effectively disabled for parents:

```rust
// src/runtime/executor.rs:795
if self.context.max_turns > 0 {
    self.llm_turn_count += 1;
    if self.llm_turn_count > self.context.max_turns { ... }
}
```

This means a provider that keeps emitting `tool_use` blocks — whether
because it's buggy, looping on an "Unknown tool" error, or genuinely
misbehaving — will run indefinitely. Every iteration persists an agent
message and a tool result to SQLite, so the loop grows the DB linearly
until either the conversation is cancelled or the process is killed.

## How this was discovered

The in-tree `mock` provider's `ReadFileToolCall` scenario asks for a
tool (`read_file`) that the Direct-mode registry doesn't expose (see
task 24684, filed in this same branch; renumbered from 24679 during
rebase to avoid colliding with main's shell-integration task that
also used 24679). A single "hello" message produced **414 agent + 414 tool
rows (+ 1 user) = 829 rows** in roughly one minute before it was
cancelled. Each agent message was a fresh row with a distinct
`mock_toolu_*` id; this is a real backend loop, not a UI dupe.

```
$ python3 ... phoenix-*.db
distinct tool_use ids: 414
agent text snippets: {"I'll read the configuration file ...": 414}
tool result snippets: {"Unknown tool: read_file": 414}
```

In the mock's case fixing task 24684 removes *this particular* loop,
but the underlying issue remains: a real Anthropic / OpenAI / Fireworks
provider that gets stuck (e.g. context window starvation, flaky tool
schema, hallucinated tool name) will do the same thing with no
backstop.

## Requirements

- [ ] Parent conversations enforce a configurable max iteration count
      on consecutive tool_use cycles (not total lifetime turns — see
      below).
- [ ] Hitting the cap halts the executor, marks the conversation idle
      with an error system message that explains what happened and how
      to resume ("Tool-use iteration limit reached after N calls. Send
      another message to continue, or Cancel and start over.").
- [ ] A user message resets the counter — long conversations with many
      turns shouldn't hit the cap just by being long; only runaway
      tool-use bursts should.
- [ ] Default cap is tunable via config/env and defaults to something
      generous but finite (suggest 100).
- [ ] Logged at `warn` when the cap is hit (`conversation_id`,
      `tool_name`, `count`).

## Open questions

- Should the cap be "N tool_uses since the last user message" or
  "N since the executor became non-idle"? The former is more forgiving
  of legitimate long plans; prefer that unless there's a reason not to.
- Should this interact with the context-exhaustion flow
  (`08513-p0-done--graceful-context-exhaustion-ui`)? Those are
  orthogonal — exhaustion is "ran out of tokens", cap is "ran out of
  patience" — but the UI affordance could be similar.
- Should sub-agents get the same reset-on-user-message semantics? They
  don't have user messages mid-run, so probably keep their current
  absolute `max_turns` behaviour.

## Related

- Task 24684 — the concrete trigger this bug fires on (renumbered
  from 24679 during rebase; collided with main's shell-integration task)
- REQ-PROJ-008 / REQ-BED-026 — existing sub-agent max_turns contract
- Task 08513 (done) — context exhaustion UI, reference for the error
  system message pattern
- Task 08014 (done) — LLM request cancellation, the existing manual
  escape hatch a user has today
