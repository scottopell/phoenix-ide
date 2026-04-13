---
created: 2026-04-12
priority: p2
status: ready
artifact: pending
---

# taskmd-panel-start-task-action

## Problem

The seed primitive (REQ-SEED-001 through -004, task 24666) shipped with
only one consumer: the terminal's shell-integration-assist button. The
other known consumer we deferred at the time is the taskmd panel.

Today, clicking a task in the tasks panel opens a viewer. There's no
one-click path from "I see this task I want to work on" to "I'm in a
new conversation scoped to the project, ready to execute it." The user
has to:

1. Read the task
2. Create a new conversation manually
3. Set the cwd
4. Pick the mode (worktree vs direct)
5. Copy-paste context from the task into the input

Five steps of repetitive ritual. The seed primitive was specifically
designed to eliminate this, and dropping a second consumer in should
be a small integration.

## Scope

- New "Start working on this task" button on each task card in the
  tasks panel (or in the task viewer, whichever surface exists today)
- Click handler builds a `ConversationSeed`:
  - `cwd` = project root (the conversation's `cwd` or `worktree_path`;
    whichever is appropriate for starting a fresh work session)
  - `conv_mode` = `worktree` (project mode default; this is different
    from the shell-integration-assist case which uses `direct`)
  - `seed_parent_id` = the current conversation (if any); if the taskmd
    panel is viewed outside a conversation, omit the parent ref
  - `seed_label` = `"Work on task NNNNN: <slug>"`
  - Draft prompt = the task markdown body plus a short orienting
    preamble telling Phoenix "Here's the task file, work on it per
    the scope, ask before doing anything destructive"
- localStorage `seed-draft:<new-id>` write + `navigate()` to the new
  conversation, matching the existing pattern from task 24666
- Breadcrumb from the seeded conversation back to wherever the task was
  started from (the parent conversation, or no breadcrumb if not inside
  one)

## Out of scope

- Auto-marking the task as `in-progress` when the seeded conversation
  starts. That's an interesting future question but it couples the
  seed primitive to taskmd's state machine, which we've been avoiding.
  Ships separately if at all.
- Multi-task batch "start working on these three tasks" — YAGNI
- A "stop working on this task" counterpart

## Related

- Parent task: 24666 (seed primitive + first consumer)
- Spec: `specs/seeded-conversations/requirements.md`
