# Introduce WorkScope and Bash Watch Foundation

## Goal

Introduce `WorkScope` as the ownership primitive for work-affine resources, then use it to lay the foundation for explicit bash watch/resume behavior without making passive process lifecycle events drive conversations.

## Core Principles

- Work-affine resources are owned by the unit of work, not necessarily by a single transcript.
- `conversation_id` identifies a transcript/runtime instance.
- `WorkScope` identifies the durable owner of resources associated with a unit of work.
- Future wake/resume behavior must be explicit, typed, and attributable to a watch contract.
- Passive child process lifecycle events are observations; they are not conversation wake events by themselves.
- Existing resource semantics should remain stable unless this task explicitly migrates them.

## Scope

Implement exactly these five steps:

1. **Add a `WorkScope` resolver**
   - Define a first-class `WorkScope` concept:
     - `WorkScope::Worktree(...)` when a conversation has a managed/branch worktree.
     - `WorkScope::Conversation(...)` otherwise.
   - Add a single resolver/helper so future resources do not each reinvent this logic.

2. **Use `WorkScope` in `bash_watch` from day one**
   - Add/design the `bash_watch` tool around explicit wake contracts.
   - Watches are owned by `WorkScope`.
   - A watch must carry enough typed intent for Phoenix to know why and how to resume the agent.
   - Ordinary bash process output/exit remains passive unless an explicit watch exists.

3. **Align tmux registry/spec with `WorkScope`**
   - Move the tmux ownership model from “per conversation” toward “per work scope.”
   - Managed/Branch continuations should share the same tmux server/session for the worktree.
   - Direct conversations continue to use their conversation id as the scope fallback.

4. **Preserve existing bash handle ownership semantics**
   - Keep ordinary bash handles scoped to the conversation/runtime that created them.
   - Do not make handle identity, tombstones, or process-control operations implicitly cross continuation boundaries.
   - Any future relationship between watches and handles must be explicit in the watch contract rather than inferred from handle existence alone.

5. **Add continuation-aware watch routing**
   - When a work-scope-owned watch fires, resolve the current active conversation for that `WorkScope`.
   - If the original conversation has continued, deliver the synthetic wake event to the continuation.
   - If the work scope is terminal/abandoned/deleted, cancel or orphan the watch rather than waking a stale conversation.

## Non-goals

- Do not make passive process output/exit wake conversations.
- Do not migrate ordinary bash handles to work-scope ownership.
- Do not broaden into unrelated notification or UI preference work.
- Do not redesign Direct-mode continuation semantics beyond the `Conversation(conversation_id)` fallback.

## Notes

The key ownership/delivery distinction to preserve is:

- `conversation_id` identifies a transcript/runtime instance.
- `WorkScope` identifies the durable owner of work-affine resources.
- The watch delivery target determines where future watch events are delivered.
- The creating conversation remains useful for audit/debug provenance.

Specs to review/update include at least `specs/bash/`, `specs/tmux-integration/`, `specs/bedrock/`, and continuation-related project/bedrock requirements.
