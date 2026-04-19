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

## Existing machinery (most of this is already built)

Research pass 2026-04-19 turned up that most pieces exist; the prototype
is mostly glue. Inventory:

- **Voice input UI.** `ui/src/components/VoiceInput/VoiceRecorder.tsx`
  wraps Web Speech API with an idle/listening/processing state machine.
  `NewConversationPage` already calls `handleVoiceFinal` /
  `handleVoiceInterim` via `useCreateConversation`. Spec:
  `specs/voice-input/`.
- **Per-conversation model.** `conversations.model` column
  (`src/db/schema.rs:25`). `POST /api/conversations/new` accepts
  `model` and validates it (`src/api/handlers.rs:473-480`). Haiku
  (`claude-haiku-4-5`) is in the registry (`src/llm/models.rs:90-99`).
- **Explore mode.** `ConvMode::Explore` (`src/db/schema.rs:129-164`);
  restricted tool set (`src/tools.rs::explore_with_sandbox`);
  Explore→Work transition at `src/runtime/executor.rs:1879`.
- **Sub-agent cheap-model pattern.** Explore sub-agents already
  auto-downgrade to the cheap model for their provider
  (`src/runtime/executor.rs:767`, `cheap_model_id_for_provider`). This
  is the exact pattern to mirror for top-level Explore.
- **propose_task + approval loop.** `src/tools/propose_task.rs` is
  intercepted at the state machine (`src/state_machine/transition.rs:
  172-220`), lands in `AwaitingTaskApproval`, and already supports
  user-feedback → re-propose cycles
  (`src/state_machine/transition.rs:1116-1138`). Approval runs
  `execute_approve_task_blocking` which writes the task file, creates
  worktree + branch, and flips to Work.
- **Seed primitive.** `POST /api/conversations/new` takes
  `seed_parent_id`, `seed_label`, allows empty `text` when seeded,
  supports `mode=auto` (managed if in a git repo, else direct).
  External callers can use this as-is.
- **Streaming.** SSE at `/api/conversations/{id}/stream`
  (`src/api/sse.rs:16-35`); chat endpoint
  `POST /api/conversations/{id}/chat` is idempotent via `message_id`
  (`src/api/handlers.rs:1130-1200`).
- **UI state phases.** Jotai atom in `ui/src/conversation/atom.ts`
  exposes `idle | awaiting_llm | llm_requesting | tool_executing |
  awaiting_user_response | ...` — natural hook for "LLM done, mic can
  re-open" in a voice loop.
- **Mode/model badges.** `ui/src/components/StateBar.tsx:115-137`
  already renders `conv_mode_label` + abbreviated model, so a turbo
  Explore conversation is visually distinct for free.

## Prototype scope

Research + design review on 2026-04-19 collapsed this into two
orthogonal changes. There is no separate "turbo" mode or flag — it's
just (a) Explore is fast by default, (b) voice works mid-conversation.

**Change 1: Explore defaults to Haiku.**
When a conversation is created with `conv_mode` resolving to Explore
and no `model` is supplied on the create request, the backend picks
`claude-haiku-4-5`. An explicit `model` still wins (so deep-Explore on
Sonnet remains possible for callers that want it). Site:
`src/api/handlers.rs:473-480` (model validation) and whichever branch
actually persists `conversation.model` — mirror the sub-agent pattern
at `src/runtime/executor.rs:767` (`cheap_model_id_for_provider`).

**Change 2: Voice works during an ongoing conversation.**
`VoiceRecorder` is currently only wired into `NewConversationPage`.
Extend it into the in-conversation input area (wherever
`ConversationPage` renders message composition). Use the full Explore
tool set — no trimming. Behavior:

- User arms the mic once per conversation (browser autoplay policy
  requires a click anyway).
- Once armed, the mic auto-opens whenever the conversation phase
  returns to `idle` (subscribe to the phase atom in
  `ui/src/conversation/atom.ts`).
- Visible listening indicator while open.
- Small debounce on re-open so the user's reaction to the previous
  turn isn't captured.
- One-click disarm.

Entry point is just "+ New → Explore" — no dedicated turbo button.
The speed is the feature.

## Resolved design decisions

Captured here so the next person (or the next session) doesn't re-open
them:

- **No separate "turbo" concept.** Two orthogonal changes — Explore
  gets Haiku as the default, and voice works mid-conversation.
- **Explicit model param still wins.** Callers who want deep-Explore
  on Sonnet can pass `model` explicitly; Haiku only applies when no
  model is supplied.
- **Tool set: unchanged.** Full current Explore tool set for Haiku
  conversations. Trim later only if latency disappoints.
- **Tool search is a non-issue.** `tool_search` only activates for
  `defer_loading` tools on models that support it
  (`src/llm/anthropic.rs:331`). Haiku gets the Explore tool list
  inline; nothing breaks.
- **Mic UX: auto-open on idle after one-click arm.** Browser
  autoplay policy forces the initial arm click anyway; after that,
  the mic re-opens on every `idle` transition until disarmed.

## Remaining open questions (implementation details)

- Debounce window on mic re-open — 250ms? 500ms? Tune empirically.
- Where exactly does the mic-arm / indicator live in the input
  area? Probably next to the send button. Look at existing patterns
  in `NewConversationPage` before inventing a new one.

## Acceptance criteria

- [ ] Creating an Explore conversation without specifying a model
      results in Haiku being used end-to-end (confirmed via StateBar
      model badge and backend logs).
- [ ] Passing an explicit `model` on conversation creation still
      overrides the Haiku default.
- [ ] `VoiceRecorder` works inside an ongoing conversation, not just
      the new-conversation page.
- [ ] Once the user arms the mic, it re-opens automatically when
      the conversation phase returns to `idle` (with a short
      debounce and a visible listening indicator).
- [ ] User can iterate verbally — push back, redirect, refine — and
      the agent re-proposes via the existing `propose_task` feedback
      loop.
- [ ] When the user approves, the task lands in `tasks/` as a normal
      `ready` task via the existing `execute_approve_task_blocking`
      path.

## Notes

- Related: 24671 (seed primitive mode auto-detection), 24666 (seed
  primitive v1) — seed primitive is the obvious external entry point
  for future non-UI callers (CLI, mobile).
- Related: `specs/voice-input/` — the voice UI is already specced and
  implemented for the new-conversation path; this task extends its
  scope to ongoing conversations.
- Related: `specs/bedrock/bedrock.allium` — Explore→Work transition
  behavior is authoritative there.
