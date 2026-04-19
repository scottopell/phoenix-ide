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
2. Audio is transcribed.
3. A turbo Explore conversation spins up on Haiku and does a shallow
   pass: looks up related tasks/specs, sanity-checks the idea against
   existing work, drafts a task file.
4. Task lands in `tasks/` as `ready` (or a new `proposed` status).
5. Later — optionally — an auto-review agent reviews proposed tasks,
   flags duplicates, suggests priority, or promotes to `ready`.

## Open questions

- Is "turbo" a new conv_mode, a flag on Explore, or just a model
  override? Leaning: model override on Explore, because the state
  machine and tool set are identical.
- Voice input: browser Web Speech API, a native client, or piping
  through `phoenix-client.py`? CLI is probably the cheapest MVP.
- Does turbo Explore get the full Explore tool set, or a reduced one
  (e.g. keyword_search + think only) to keep latency predictable?
- Auto-review: separate background conversation per proposed task, or
  a batched digest? Likely batched to avoid runaway token spend.

## Acceptance criteria (rough)

- [ ] Explore mode accepts a model override; Haiku works end-to-end.
- [ ] `phoenix-client.py` (or equivalent) supports voice-in → task-out
      in a single invocation.
- [ ] Proposed tasks are distinguishable from human-authored ones
      (status, frontmatter marker, or dedicated dir).
- [ ] Auto-review path is stubbed or tracked as a follow-up task.

## Notes

- Related: 24671 (seed primitive mode auto-detection), 24666 (seed
  primitive v1) — the seed primitive is probably the right entry
  point for "spawn a turbo Explore from outside the UI".
- Related: subagents spec — turbo Explore is conceptually close to
  a short-lived Explore sub-agent.
- Keep auto-review as a separate task once the capture path exists;
  don't block idea-capture on reviewer tooling.
