# Introduce Watchable WorkScope Jobs

## Goal

Make long-running, resume-worthy work coherent by introducing a generic WorkScope-owned watch system over durable job subjects, starting with `tmux_run`, while preserving ordinary `bash` handle semantics.

## Problem

The current `bash_watch` foundation establishes explicit wake contracts and continuation-aware routing, but ordinary bash handles remain conversation/runtime-scoped. A watch can outlive the conversation that created a bash handle, but if the underlying handle/process is gone or unavailable to the continuation, the watch has nothing durable to observe.

This creates a misleading pit: `bash_watch` sounds like the right tool for “run this and wake me later,” but ordinary bash handles are intentionally the wrong substrate for durable cross-continuation work.

## Direction

Introduce a generic `watch` model over typed watch subjects. Watches remain owned by `WorkScope`, but each subject declares its observation durability. Long-lived shell work should flow through `tmux_run` as a WorkScope-owned/watchable job; ordinary `bash` handles remain conversation-local and non-durable.

The intended user/agent story should be:

- Use `bash` for ordinary short-lived, conversation-local commands.
- Use `tmux_run` for long-running or inspectable work.
- Use `watch` on a `tmux_run` run id when Phoenix should resume once a condition becomes true.
- Do not use ordinary `bash` handles as the durable cross-continuation path.

## Scope

Implement these pieces:

1. **Define a generic `watch` contract model**
   - Watches are owned by `WorkScope`.
   - Watches carry:
     - typed subject
     - typed trigger
     - typed wake intent
     - creating conversation provenance
     - durability/recovery classification
   - Wake delivery resolves the current active conversation for the `WorkScope`.
   - If the scope is terminal/deleted/unrecoverable, cancel/orphan/drop the watch with a logged reason rather than waking stale work.

2. **Make `tmux_run` produce watchable run ids**
   - Add a Phoenix-level `run_id` distinct from tmux `window_id`.
   - Preserve `window_id` for tmux inspection.
   - Track metadata for each run:
     - `run_id`
     - `WorkScope`
     - tmux target/window
     - command/name
     - creating conversation id
     - status if known
   - Prefer this as the durable long-running shell-job substrate.

3. **Allow `watch` to subscribe to `tmux_run` subjects**
   - Start with triggers such as:
     - window/process exited
     - output contains text
   - Firing a watch must be caused by an explicit watch contract, not passive tmux output alone.
   - On fire, route to the active continuation for the `WorkScope`.

4. **Rescope or replace `bash_watch`**
   - Rename/subsume `bash_watch` into the generic `watch` tool before it becomes the long-term public abstraction, or explicitly restrict it to same-conversation/non-durable use.
   - If `watch` supports `BashHandle` subjects, mark them non-durable:
     - no Phoenix-restart survival
     - no implicit cross-continuation control/inspection
     - cancel/orphan when the source handle disappears
   - Tool descriptions and system prompt guidance should steer agents to `tmux_run` + `watch` for long-lived work.

5. **Persist enough metadata for recovery**
   - Persist watch contracts and watchable tmux run metadata where needed.
   - On Phoenix restart, attempt to reattach to tmux scope/window and resume observation.
   - If the subject cannot be recovered, mark the watch cancelled/orphaned with an explicit reason.

6. **Update specs**
   - `specs/bash/`: ordinary bash handles remain conversation-scoped and non-durable.
   - `specs/tmux-integration/`: tmux runs can be WorkScope-owned watchable jobs.
   - `specs/bedrock/`: watch firing routes to the active continuation.
   - Add or expand `specs/watch/` for:
     - subject/trigger/wake contract model
     - durability matrix
     - restart/orphan/cancel behavior
     - no passive lifecycle wakeups without explicit contracts

## Non-goals

- Do not migrate ordinary bash handles to WorkScope ownership.
- Do not make passive bash/tmux output or exit wake conversations without an explicit watch.
- Do not make arbitrary shell commands durable unless launched through the watchable WorkScope job path.
- Do not add UI notification preferences in this task.
- Do not redesign Direct-mode continuation semantics beyond the `WorkScope::Conversation(conversation_id)` fallback.

## Design principles

- `conversation_id` identifies a transcript/runtime instance.
- `WorkScope` identifies the durable owner of work-affine resources.
- Observation authority and delivery target are separate concepts:
  - the subject determines whether Phoenix can still observe the condition;
  - the `WorkScope` determines where the wake is delivered.
- Ordinary bash handles stay simple and local.
- Long-lived watched shell work should be launched through the WorkScope-owned path.
