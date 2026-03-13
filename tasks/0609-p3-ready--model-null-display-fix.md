---
created: 2026-03-13
number: 609
priority: p3
status: ready
slug: model-null-display-fix
title: "StateBar shows 'Model: null' when no model explicitly set"
---

# StateBar shows "Model: null" when no model explicitly set

## Problem

Conversations created without an explicit model selection show "null" in the
StateBar model display. The backend falls back to the default model for LLM
calls, but the `model` field on the Conversation record remains null.

Observed in QA screenshots: StateBar shows "Model: null" instead of the
actual model being used (e.g., "claude-haiku-4-5").

## What to Do

In the StateBar or wherever `conversation.model` is displayed, fall back to
a display value when null:
- Option A: Show "default" or the actual default model name from the models API
- Option B: Backfill `conversation.model` on the backend when creating the
  conversation (resolve the default at creation time)

Option B is better -- the conversation record should reflect what model is
actually being used. Check `create_conversation_with_project` and the
`/api/conversations/new` handler.

## Acceptance Criteria

- [ ] StateBar never shows "null" for the model name
- [ ] New conversations store the resolved model name
- [ ] Existing conversations with null model show a reasonable fallback
