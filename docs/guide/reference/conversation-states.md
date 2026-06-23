---
title: Conversation states
summary: The states a conversation moves through — which mean the agent is busy, and which are waiting on you.
category: reference
keywords: [state, status, busy, idle, thinking, error, conversation states]
related: [concepts/conversations.md, reference/managed-lifecycle-states.md, reference/glossary.md]
---

# Conversation states

> **At a glance:** a conversation is always in one state. **Busy** = the agent is
> working (you can cancel or [steer](../howto/steer-a-running-agent.md), not
> edit). The rest are waiting on you or are read-only.

## Busy — the agent is working

| State | What's happening |
|-------|------------------|
| Thinking | awaiting / streaming the LLM's response |
| Running a tool | executing a tool (bash, patch, browser, …) |
| Awaiting sub-agents | waiting for spawned [sub-agents](../concepts/sub-agents.md) to finish |
| Summarizing | context is full — writing a continuation summary |
| Cancelling | stopping at your request |
| Recovering | recovering from an error mid-run |

## Waiting on you

| State | What's happening |
|-------|------------------|
| Idle | ready for your next message (you can switch model here — or in Error) |
| Awaiting approval | a proposed task is waiting for your **Approve** / **Send Feedback** / **Discard** |
| Awaiting your answer | the agent asked a question and needs your pick |
| Error | a turn failed; retry from idle — you can switch model here to recover (e.g. from quota/overload) |

## Read-only (terminal)

| State | What's happening |
|-------|------------------|
| Context exhausted | the context window filled; the conversation is read-only |
| Continued | work was handed to a successor — act on the [continuation](../concepts/chains.md) |
| Done | completed or abandoned |

## Related

- [Conversations](../concepts/conversations.md) — the idle ⇄ busy model
- [Managed lifecycle states](managed-lifecycle-states.md) — the Done? bar by state
- [Glossary](glossary.md) — canonical terms
