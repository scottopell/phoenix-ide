---
created: 2026-04-07
priority: p1
status: ready
artifact: src/runtime/executor.rs
---

# Give sub-agents a final turn to submit results on turn limit

## Problem

When a sub-agent hits its turn limit, Phoenix sends UserCancel which
transitions to Failed and returns SubAgentOutcome::Failure to the parent.
All work the sub-agent did is discarded -- the parent sees only an error
message. This wastes all tokens spent on the sub-agent's turns.

## What to change

In the turn-limit handler in executor.rs (~line 730), instead of sending
UserCancel, inject a synthetic system message telling the sub-agent to
wrap up and submit its findings, then allow one final LLM turn:

1. Detect turn limit is about to fire (turn count == max_turns)
2. Inject a system message: "You have reached your turn limit. Please
   call submit_result now with whatever findings you have so far."
3. Allow one more LLM turn -- the model should call `submit_result`
   which triggers the normal Success path
4. If the final turn does NOT produce `submit_result` (model calls
   another tool or produces only text), fall back to extracting the
   last assistant text and returning it as Success
5. If no useful output at all, return Failure as current behavior

This mirrors how a human would handle it: "time's up, give me what
you have." The sub-agent gets a chance to synthesize its work into a
coherent response rather than being abruptly killed.

## Implementation notes

- The system message injection can reuse the existing PersistMessage
  effect or be injected directly into the LLM request's message history
- The "one more turn" is just not sending UserCancel -- let the normal
  LLM request flow proceed, but set a flag so the NEXT turn check
  hard-stops
- `submit_result` is already in the sub-agent tool registry

## Edge cases

- Sub-agent calls a tool instead of submit_result on the final turn:
  extract last assistant text as fallback (walk backward through
  messages to find most recent assistant text block)
- Sub-agent errors on the final LLM call: return Failure

## Done when

- [ ] Sub-agent gets a final turn with a wrap-up prompt at turn limit
- [ ] Model calls submit_result with partial findings in most cases
- [ ] Fallback extracts last assistant text if submit_result not called
- [ ] Parent receives useful content from turn-limited sub-agents
- [ ] Existing sub-agent tests still pass
