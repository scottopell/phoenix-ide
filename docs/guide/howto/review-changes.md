---
title: Review changes
summary: Read the agent's edits in the diff viewer and hand back line-level notes as feedback.
category: howto
keywords: [review, diff, changes, annotate, notes, feedback, viewer]
related: [reference/managed-lifecycle-states.md, concepts/conversations.md, howto/run-a-managed-task.md]
---

# Review changes

Read what the agent changed and leave line-level feedback it can act on, without
leaving the conversation.

## Before you start

The agent has made (or is making) edits — typically in Work or Branch mode.

## Steps

1. **Open the diff.** In the **Done?** bar, click **View Diff**. The diff viewer
   (**Worktree diff**) shows **Committed changes** and **Uncommitted changes**
   versus the base branch; toggle split/unified from its header.
2. **Read a file in full** by opening it in the viewer. The side slot holds one
   of diff / file / browser at a time — opening one closes the others.
3. **Annotate a line.** Click a line and add a note (**Add your note… (Cmd/Ctrl+Enter
   to save)**). Notes collect in the **Notes** panel, each anchored to its line.
4. **Insert, then send.** **Send All** drops your notes into the composer
   (focused and ready) — it does **not** deliver them on its own; **press Send**
   to hand them to the agent. **Clear All** discards them.

## Result

Once you send, the agent receives your annotations tied to specific lines and
iterates — the same notes mechanism the [task-approval](../reference/managed-lifecycle-states.md#task-approval-reader)
review uses.

## See also

- [Managed lifecycle states](../reference/managed-lifecycle-states.md) — View Diff in the Done? bar
- [Run a managed task](run-a-managed-task.md) — where review fits the lifecycle
- [Conversations](../concepts/conversations.md) — steering vs. annotating
