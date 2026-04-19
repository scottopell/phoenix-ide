---
created: 2026-04-19
priority: p3
status: ready
artifact: pending
---

# explore-defaults-to-haiku

## Summary

When a new conversation is created in Explore mode and the caller does
not specify a model, the backend should default to `claude-haiku-4-5`
instead of the registry-wide default. Explicit `model` on the create
request still wins.

## Context

Explore is a read-only planning mode. Today it uses whatever the
registry default is (currently a Sonnet-tier model), which is overkill
for most planning and makes the iterative "think out loud, push back,
refine" loop feel sluggish. Haiku is fast enough that the planning
phase can feel conversational.

The pattern already exists for Explore *sub-agents*:
`src/runtime/executor.rs:767` calls `cheap_model_id_for_provider()` to
downgrade sub-agents to the cheap model for their provider. This task
extends the same defaulting to top-level Explore conversations at
create time.

## Scope

- When `POST /api/conversations/new` resolves `conv_mode` to Explore
  AND `model` is absent on the request, store
  `claude-haiku-4-5` as `conversation.model` (or resolve the cheap
  model for the configured provider, mirroring the sub-agent helper).
- If the caller passes an explicit `model`, do not override — validate
  and persist as today.
- Verify: `StateBar` model badge shows the Haiku label, backend logs
  confirm Haiku is the model on the first LLM turn.

## Out of scope

- Any UI change. The existing mode/model badge
  (`ui/src/components/StateBar.tsx:115-137`) already renders the
  resolved model; no new indicators needed.
- Voice behavior changes. Tracked separately in task 24688.
- Changing the default for Work or Direct mode.
- Trimming the Explore tool set. Haiku gets the full current Explore
  tools; `tool_search` only activates for `defer_loading` tools on
  supporting models (`src/llm/anthropic.rs:331`), so there is no
  compatibility issue.

## Acceptance criteria

- [ ] Creating an Explore conversation via `POST /api/conversations/new`
      with no `model` field results in `conversation.model =
      claude-haiku-4-5` in the DB.
- [ ] Creating an Explore conversation with an explicit `model` stores
      that model unchanged.
- [ ] Work / Direct conversations are unaffected.
- [ ] First LLM dispatch uses the resolved model (logs confirm).

## Notes

- Primary site: `src/api/handlers.rs:473-480` (model validation) and
  the adjacent persistence path that writes `conversation.model` into
  the DB (`src/db.rs:356-387`).
- Pattern to mirror: `cheap_model_id_for_provider()` at
  `src/runtime/executor.rs:767`.
- Registry: `claude-haiku-4-5` is at `src/llm/models.rs:90-99`.
- Related task: 24688 (persistent voice listening) — the two are
  orthogonal but ship together experientially.
