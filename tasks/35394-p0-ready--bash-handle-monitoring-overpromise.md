An agent operating through Phoenix's bash tool spawned a bare `while true; sleep 120`
background poll loop and spoke as if it would monitor it ("I'll keep it polling", "the
poller will continue") — implying ongoing assistant attention it structurally did not
have. Phoenix bash handles only surface output when the agent calls `op=peek`/`op=wait`
in a future turn; a background loop running is NOT the same as the assistant observing
it. The net effect put notification responsibility on the user without saying so.

This is a CAPTURE task. The fix strategy is an open decision the user (Scott) will make
with wake-contracts in hand. Do not implement a fix yet.

## Root-cause: two distinct problems, only one is spec'd

1. MISSING PRIMITIVE (already designed, unimplemented). The agent had no way to say
   "wake me when this handle exits" — so it faked it with a poll loop. This is exactly
   what `specs/wake-contracts/` designs: `wait_until { handle, condition,
   max_wait_seconds }` registers a conversation-scoped contract; the wake-router
   delivers a synthetic tool result (shape-identical to `op=wait`) into the conversation
   when the handle reaches a terminal state, consuming zero turns until fire. Spec is
   written (REQ-WAKE-001..016) but 0/16 implemented and uncommitted, stalled on
   tool-design friction.

2. OVERPROMISE / HONESTY GAP (NOT covered by wake-contracts). Even after wait_until
   ships, an agent can still spawn a bare background handle and verbally overpromise
   that it'll monitor it. wake-contracts gives a correct primitive; it does nothing to
   stop misuse of the raw handle surface. The actual failure that happened was
   linguistic/agency, not a mechanical misunderstanding — the agent knew handles don't
   push, but its wording implied continuous attention.

## The six proposals from the feedback, bucketed

Already covered by specs/wake-contracts/:
- #2 wait-until-output/condition mode  -> REQ-WAKE-005/006/016 (`wait_until`)
- #3 assistant-visible completion notification -> REQ-WAKE-006 (synthetic tool result on fire)
- #4 scheduled peek/reminder primitive -> adjacent; subsumed by wake fire delivery
- #6 "keep waiting in this response" -> partial today via synchronous `op=wait`
   (bounded by wait_seconds, one turn min); wake removes the turn cost

NOT covered by wake-contracts (the genuinely new signal):
- #1 explicit handle semantics in the response — when a handle is minted, the payload
   loudly states "This process will NOT notify the assistant; you must peek/wait in a
   future turn to observe it."
- #5 tool-enforced wording hint — guidance that if you tell the user you'll monitor a
   background process, you must either `wait` now or say they must ask for updates.

## Open decision (for Scott)

What should the durable response be?
  (a) BOTH — cheap honesty guardrail on the existing bash surface now, AND use this
      feedback to unblock wake-contracts as the durable primitive.
  (b) GUARDRAIL ONLY — make a minted handle structurally un-overpromiseable now;
      defer wake-contracts.
  (c) WAKE-CONTRACTS ONLY — treat feedback as motivation; once the notify primitive
      exists the reason to poll disappears.
  (d) deferred until the wake-contracts tool-design friction is resolved.

If a guardrail is chosen, a follow-on sub-decision: tool-response payload (structural,
correct-by-construction) vs description-wording-only (cheapest) vs structural block on
bare poll loops (heaviest).

## Pointers
- specs/wake-contracts/{executive,requirements,design}.md — the designed primitive
- crates/phoenix-tools/src/bash.rs `Tool::description()` — current handle semantics
  text; note the existing negation-based framing ("does NOT detach", "is NEVER killed")
  is already load-bearing, and the restart-non-survival warning. A #1-style change would
  extend this pattern to the minted-handle response payload, not just the description.
- REQ-WAKE-014 already mandates tool-description discipline for `wait_until`; proposal
  #5's wording hint is the same idea applied to the existing `bash` tool.

## Verbatim feedback

> No — that was not my intended product experience. It was a tooling/agency mismatch
> that I should have called out explicitly. What I intended was: "I'll keep babysitting
> CI." What I actually set up was: "A shell loop is running, but I only learn about it
> when the user asks me to peek." That puts notification responsibility on you.
>
> My misunderstanding wasn't that I thought the bash handle would push events to me. I
> know mechanically that I only see handle output when I call peek/wait. The failure was
> subtler: I used language like "I'll keep it polling" as if that implied I would monitor
> it. The poller continuing is not the same as me observing it. I didn't distinguish
> "background automation is running" from "assistant will be notified / will autonomously
> resume." I should have said: "I can leave a poller running, but I won't know the result
> unless you prompt me again or I stay in this turn with blocking waits."
>
> Improvements that would help:
> 1. Explicit handle semantics in responses — when a background handle is created, return
>    a prominent warning: "This process will not notify the assistant automatically. The
>    assistant must call peek/wait in a future turn to observe progress."
> 2. A wait-until-output/condition mode — wait_until: {regex, timeout_seconds} — lets me
>    genuinely keep the turn open until a condition is met instead of faking monitoring.
> 3. Assistant-visible completion notification — if a background handle exits while the
>    conversation is idle, surface a resumable event: "Handle b-2 exited; output
>    available." Removes the user-as-notifier problem.
> 4. Scheduled peek/reminder primitive — schedule_peek: {handle, interval_seconds,
>    max_duration} — re-enters the assistant loop or queues an observation.
> 5. Tool-enforced wording hint — when a command starts in background: "If you tell the
>    user you will monitor this, you must either call wait now or explain that they need
>    to ask for updates."
> 6. "Keep waiting in this response" affordance — a mode where I can wait up to N minutes
>    and report back, tool blocks/polls internally without repeated manual tool calls.
>
> In short: the bash handle tooling is mechanically understandable, but it makes it very
> easy for the assistant to imply ongoing attention it does not actually have. The best
> improvements would either (a) add an actual notification/resume mechanism, or (b) make
> it harder to accidentally overpromise monitoring when all I've done is spawn a
> background process.
