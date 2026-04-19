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

What's actually missing to get a working turbo Explore prototype:

1. **Pick Haiku automatically for top-level Explore conversations.**
   Either (a) mirror the sub-agent pattern by auto-selecting Haiku in
   `handlers.rs` when `conv_mode` resolves to Explore and no model was
   passed, or (b) add an explicit `turbo: bool` flag on the create
   request. (a) is fewer knobs; (b) is more explicit. Open question.
2. **Reuse `VoiceRecorder` inside the conversation input area**, not
   just on `NewConversationPage`. Today mid-conversation voice input
   requires typing. For the interactive loop we need continuous
   voice throughout.
3. **"Auto-listen when idle" toggle.** Subscribe the
   `VoiceRecorder` start to the conversation phase returning to
   `idle`. Default on for turbo conversations, off otherwise.
4. **Entry point.** A "Turbo voice" button next to `+ New` in
   `ConversationList.tsx:94` that creates a conversation with
   `mode=auto`, `model=claude-haiku-4-5`, and flips the voice/auto-
   listen toggles on by default. (Or a URL param like
   `/new?turbo=1` interpreted by `NewConversationPage`.)

That's the MVP. Everything else — `phoenix-client.py` voice support,
mobile entry, hotkey-to-turbo — is follow-up.

## Open questions

- **Turbo flag vs. model inference.** If Explore is always Haiku,
  turbo stops being a separate concept. Is there ever a reason to
  want Explore on Sonnet? (Deep explore on a large codebase, maybe.)
  If yes, turbo is an explicit flag; if no, just always pick Haiku
  for Explore.
- **Haiku + tool_search.** `claude-haiku-4-5` is marked
  `supports_tool_search: false` (`src/llm/models.rs:98`). Does the
  Explore tool set rely on tool_search? If so, turbo needs either
  a different fallback or the Explore registry needs to avoid
  tool_search when the model can't do it.
- **Tool set for turbo.** Keep full Explore tools, or trim to
  `keyword_search` + `think` + `propose_task` for predictable
  latency? Full set first, trim later based on real use.
- **Auto-listen barge-in.** If the mic re-opens the instant state
  hits `idle`, the user might accidentally capture their own
  reaction to the previous turn. Needs a short debounce and
  probably a visible "listening" indicator.
- **Voice across page reloads.** Web Speech API is session-scoped.
  Reconnecting SSE after a reload is handled; re-arming the mic
  probably needs explicit user click (browser autoplay policies).

## Acceptance criteria (rough)

- [ ] Creating an Explore conversation results in Haiku being used
      end-to-end (confirmed via StateBar model badge and backend
      logs).
- [ ] `VoiceRecorder` works inside an ongoing conversation, not just
      the new-conversation page.
- [ ] In turbo mode, the mic re-opens automatically when the
      conversation phase returns to `idle`.
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
