---
title: Steer a running agent
summary: Send a message while the agent is busy — it queues and is delivered at the next turn, no interruption.
category: howto
keywords: [steer, steering, queue, busy, redirect, follow-up]
related: [concepts/conversations.md, reference/conversation-states.md]
---

# Steer a running agent

You don't have to wait for the agent to finish to add direction. A message sent
while it's busy is **queued** and handed to it at the next safe point — a way to
course-correct without cancelling.

## Before you start

The agent is working (a [busy](../reference/conversation-states.md) state).

## Steps

1. **Type while it works.** The composer stays open; its placeholder reads
   **Agent working… send to queue a follow-up**.
2. **Send.** The message is queued, not rejected. It appears in the transcript
   marked **⏳ Queued** (*"Queued — will send when conversation is free"*). Up to
   **5** messages can wait in the queue; a 6th is refused (*"Steering queue is
   full…"*) until a queued one is delivered.
3. **Change your mind?** Cancel a queued message with its **×** (*"Cancel queued
   message"*) before it's delivered.
4. **It lands at the next boundary.** Phoenix drains the queue when the
   conversation reaches the end of a turn — or between tool rounds mid-turn — and
   the agent picks it up as your next instruction.

## Result

The agent adjusts to your steer at the next turn, without losing the work in
flight.

## See also

- [Conversations](../concepts/conversations.md) — steering vs. cancelling
- [Conversation states](../reference/conversation-states.md) — which states are "busy"
