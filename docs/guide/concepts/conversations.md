---
title: Conversations
summary: The primary unit of work — a message thread with the agent, bound to a directory and a mode, that moves between idle and busy.
category: concepts
keywords: [conversation, thread, idle, busy, cancel, steer, persistence, continuation]
related: [concepts/modes.md, concepts/chains.md, reference/glossary.md]
---

# Conversations

A **conversation** is a single thread with the agent, bound to a working
directory and running in a [mode](modes.md). It is Phoenix's primary unit of
work — tasks, tools, terminals, and chains all hang off one conversation.

A conversation is always either **idle** or **busy**:

```
 idle ──you send──▶ busy ──agent replies, runs tools──▶ idle
                     │
                     └─ cancel it, or steer it with another message
```

## What's in one

- **A directory and a mode.** Every conversation is pinned to a working directory
  and a [mode](modes.md) that sets how much the agent can do.
- **A typed thread.** Your messages, the agent's replies, and the tool calls it
  makes, in order — persisted and crash-recoverable. Close the tab, lose the
  network, come back: the thread and your place in it are restored.
- **Live state.** While the agent is busy you can cancel it, or *steer* it — send
  another message that's picked up at the next turn rather than interrupting. If
  a turn errors, the conversation offers to resume.
- **Continuable.** A conversation can be continued into a successor; a run of
  continuations becomes a [chain](chains.md).

## What you'll see

The sidebar lists your conversations; opening one shows the message history with
a composer beneath it and a state bar reflecting whether the agent is idle,
working, or disconnected. You start new conversations from the sidebar.

> **Remember:** the conversation is the unit of work *and* of persistence — mode,
> state, directory, and history all belong to it, not to the app. It survives
> crashes and reconnects.

## See also

- [Modes](modes.md) — the freedom level a conversation runs in
- [Chains](chains.md) — runs of continued conversations
- [Glossary](../reference/glossary.md) — conversation, continuation
