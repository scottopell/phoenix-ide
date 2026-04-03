---
created: 2026-02-27
priority: p2
status: done
artifact: completed
---

# Zombie Sub-Agent Runtime Leak

## Problem

The executor `run()` loop in `src/runtime/executor.rs` exits only when all `event_tx` senders are dropped. Sub-agent runtimes hold a sender in their `ConversationHandle` that is never dropped when the conversation ends (reaches a terminal state like `Completed`, `Failed`, etc.).

This means terminated sub-agent executors stay alive in memory indefinitely — goroutine-style leaks. Each spawned sub-agent that finishes its work but whose `ConversationHandle` is still held somewhere keeps the tokio task and associated state in memory.

## Reproduction

Start a conversation that spawns sub-agents. After all sub-agents complete, inspect memory or tokio task count — the sub-agent executor tasks are still running.

## Fix Direction

When a conversation reaches a terminal state, the executor should drop its `event_tx` (or signal the run loop to exit). Options:

1. After executing effects that land in a terminal state, break out of the `run()` loop.
2. Check `new_state.is_terminal()` after each transition and exit if true.
3. Have the `ConversationHandle` drop its sender when it receives a terminal state notification.

The simplest fix is likely option 2: add a check in the executor's main event loop after applying a transition, and exit the loop if the new state is terminal.

## Context

Discovered during `spawn_agents` deadlock investigation (fixed in tasks leading up to this). Not causing the deadlock, but is a correctness/resource issue worth tracking.
