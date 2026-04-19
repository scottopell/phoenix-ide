---
created: 2026-04-19
priority: p3
status: ready
artifact: pending
---

# turbo-explore-voice-idea-capture

## Summary

Idea: a "turbo" variant of Explore mode backed by Haiku, optionally driven
by voice input, so a user can quickly sketch an idea out loud, have it
roughly explored, and land a task file — without the friction of opening
the UI, typing, and waiting on Sonnet/Opus-scale latency.

## Context

Today Explore mode uses the same model tier as Work mode. That is fine
for deep-dive exploration but overkill for the "I just had an idea in the
shower, capture it before I lose it" workflow. Haiku is fast enough that
a voice-to-task round trip could feel instantaneous.

The end-to-end vision:

1. User triggers voice capture (hotkey, CLI, or mobile).
2. Audio is transcribed and streamed into a turbo Explore conversation
   on Haiku.
3. Turbo Explore looks up related tasks/specs, sanity-checks the idea
   against existing work, and drafts a task file — *interactively*.
   The point of Haiku is that the user can push back verbally ("no,
   that's the wrong direction", "what about X?", "merge with task
   24670") and iterate on the plan without LLM latency breaking the
   flow. This is the planning/exploration phase, just fast enough to
   stay conversational.
4. When the user is satisfied, the task lands in `tasks/` as `ready`.
   No new task type or status — it's a normal task, just authored via
   a faster front door.

## Open questions

- Is "turbo" a new conv_mode, a flag on Explore, or just a model
  override? Leaning: model override on Explore, because the state
  machine and tool set are identical.
- Voice input: browser Web Speech API, a native client, or piping
  through `phoenix-client.py`? CLI is probably the cheapest MVP, but
  the interactive loop wants duplex audio, which pushes toward the
  browser or a native client.
- Does turbo Explore get the full Explore tool set, or a reduced one
  (e.g. keyword_search + think only) to keep latency predictable
  enough for a conversational loop?
- How is the interactive refinement surfaced? Same conversation,
  user speaks again and the draft updates in place? Or explicit
  "refine" turns with a visible diff of the task file?

## Acceptance criteria (rough)

- [ ] Explore mode accepts a model override; Haiku works end-to-end.
- [ ] Voice input drives a turbo Explore conversation (client TBD).
- [ ] User can iterate verbally — push back, redirect, refine — and
      the draft task updates in place without the round trip feeling
      slow.
- [ ] When the user commits, the task lands in `tasks/` as a normal
      `ready` task.

## Notes

- Related: 24671 (seed primitive mode auto-detection), 24666 (seed
  primitive v1) — the seed primitive is probably the right entry
  point for "spawn a turbo Explore from outside the UI".
- Related: subagents spec — turbo Explore is conceptually close to
  a short-lived Explore sub-agent.
