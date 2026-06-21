---
title: Getting started
summary: Open your first conversation — pick a directory, send a request, watch the agent work.
category: howto
keywords: [getting started, new conversation, first, directory, send, onboarding]
related: [concepts/conversations.md, concepts/modes.md, howto/run-a-managed-task.md, howto/compose-with-references.md]
---

# Getting started

Your first conversation, end to end. A [conversation](../concepts/conversations.md)
is the unit of work — you point it at a directory, describe what you want, and the
agent goes.

## Before you start

Phoenix is open in your browser.

## Steps

1. **Start a new conversation** from the sidebar.
2. **Set the working directory** to the folder you want to work in. A **git
   repository** unlocks the **Workflow** chooser (plan-first or branch work — see
   [Modes](../concepts/modes.md)); a non-git folder runs in **Direct** mode with
   full access.
3. **Pick a model** if you don't want the default.
4. **Type your request.** The composer hints at `/` for skills and `@` to include
   files — see [Compose with references](compose-with-references.md).
5. **Press Send** (it reads `Creating...` while it sets up). The conversation
   opens and the agent starts working.
6. **Watch and steer.** While the agent is busy you can cancel it, or send another
   message to steer — it's picked up at the next turn (see
   [Conversations](../concepts/conversations.md)).

## Result

A running conversation in your chosen directory and mode. From here: for a change
you want planned and approved first, follow
[Run a managed task](run-a-managed-task.md); otherwise just keep chatting in
Direct mode.

## Troubleshooting

- **No Workflow chooser appears.** The directory isn't a git repo (Direct mode
  only), or branch metadata is still loading.
- **Send is disabled.** You need message content and, for an isolated workflow, a
  resolved starting branch — see
  [Run a managed task](run-a-managed-task.md#troubleshooting).

## See also

- [Conversations](../concepts/conversations.md) — what you just created
- [Modes](../concepts/modes.md) — how much freedom the agent gets
- [Run a managed task](run-a-managed-task.md) — the plan-first flow
