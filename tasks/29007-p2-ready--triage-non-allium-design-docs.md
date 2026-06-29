# Triage legacy design docs without Allium

## Goal

Decide which legacy `design.md` files that do not have an Allium layer can be retired into lightweight spEARS v2 artifacts, and which need future Allium distillation before their behavior prose can be safely removed.

## Specs to triage

- `agent-identity`
- `analytics`
- `api`
- `ask-user-question`
- `chains`
- `command-palette`
- `commission-review`
- `conversation-retrieval`
- `conversation-ui`
- `deployment-info`
- `file-explorer`
- `keyboard-interaction`
- `keyword_search`
- `process-inspector`
- `prose-feedback`
- `simple_client`
- `skills-ui`
- `tasks-ui`
- `think`
- `voice-input`
- `work-scope-ui`

## Classification rubric

For each spec, classify it into one of these buckets:

1. **No Allium needed.** CRUD endpoint, simple UI surface, pure transformation, static documentation, or low-state tool. Plan to move rationale/non-goals into ADRs or requirements/executive and retire `design.md` without a formal behavior layer.
2. **Allium distillation needed.** State machine, lifecycle, multi-step operation, partial failure behavior, concurrency/race boundary, or cross-spec contract. Create a follow-up task to distill `.allium` before retiring `design.md`.
3. **Spec drift investigation needed.** The design doc appears contradicted by code, requirements, or completed tasks. Create a focused bug/spec-drift task before migration.
4. **Already obsolete.** The design doc describes deprecated behavior and can be removed only after requirements/executive/ADRs make that explicit.

## Deliverables

- A triage table covering every listed spec with:
  - bucket;
  - reason;
  - recommended next action;
  - risk level;
  - whether any code comments cite the design doc.
- Follow-up tasks for every bucket-2 or bucket-3 spec.
- Lightweight migration edits for bucket-1 specs only when they are obvious and small.

## Non-goals

- Do not distill all Allium specs in this task.
- Do not delete design docs without proving their content has a v2 home.
- Do not rewrite core high-risk specs already covered by task 29005 unless triage shows they belong here instead.

## Acceptance criteria

- Every non-Allium legacy design doc is classified.
- Follow-up work is concrete enough for an agent to execute without repeating the inventory from scratch.
- Any spec touched is left more v2-aligned and passes relevant checks.
- `./dev.py check` passes if edits go beyond task-file triage.
