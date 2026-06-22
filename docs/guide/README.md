---
title: Phoenix User Guide
summary: Learn to drive Phoenix — conversations, modes, tasks, sub-agents, and the tools the agent uses on your behalf.
category: landing
keywords: [guide, help, getting started, overview]
related: [concepts/modes.md, howto/run-a-managed-task.md, reference/glossary.md]
---

# Phoenix User Guide

Phoenix is an LLM-powered coding agent. You hold a **conversation** with it; it
reads and edits your code through **tools**, in a **mode** that decides how much
freedom it has. This guide teaches you to drive it well.

It is organized in three layers. Start wherever your question lives:

- **[Concepts](#concepts)** — *what a thing is and why it exists.* Read these
  first; they're short and they name the vocabulary the rest of the guide uses.
- **[How-to](#how-to)** — *how to accomplish a goal,* step by step.
- **[Reference](#reference)** — *exact flags, keys, states, and limits.* Dense
  lookup cards for when you already know what you want.

> This page and every page under it render both on GitHub and inside Phoenix —
> open the in-app guide from the command palette (**Open User Guide**) or
> **Settings → User guide**. The [`SUMMARY.md`](SUMMARY.md) manifest is the table
> of contents both renderers read.

Linked entries are written; the rest are `*(planned)*` and listed in
[`SUMMARY.md`](SUMMARY.md). (Planned pages aren't linked here — a link to an
unwritten page would 404 on GitHub.)

## Concepts

- [Overview](concepts/overview.md) — the big picture
- [Conversations](concepts/conversations.md) — the primary unit of work
- [Modes](concepts/modes.md) — Direct, Explore, Work, Branch
- [Tasks](concepts/tasks.md) — the plan that gates Explore → Work
- [Sub-agents](concepts/sub-agents.md) — child conversations, run in parallel
- [Skills](concepts/skills.md) — reusable instruction sets
- [Workspace](concepts/workspace.md) — the worktree and its live resources
- [Permissions](concepts/permissions.md) — the deny layer over tool calls
- [Chains](concepts/chains.md) — continuation runs, queryable as a unit
- [Providers & models](concepts/providers.md) — picking the LLM

## How-to

- [Getting started](howto/getting-started.md) — your first conversation
- [Run a managed task](howto/run-a-managed-task.md) — Explore → approve → Work → PR
- [Spawn sub-agents](howto/spawn-sub-agents.md) — fan work out in parallel
- [Compose with references](howto/compose-with-references.md) — `@file` `/skill` `./path`
- [Use the terminal](howto/use-the-terminal.md) — persistent shell + tmux
- [Review changes](howto/review-changes.md) — diff viewer + line notes
- [Steer a running agent](howto/steer-a-running-agent.md) — queue a message mid-run
- [Search conversations](howto/search-conversations.md) — find past work
- [Voice input](howto/voice-input.md) — dictate a message
- Share read-only — *(planned)*

## Reference

- [Managed lifecycle states](reference/managed-lifecycle-states.md) — approval & Done? bar states
- [Glossary](reference/glossary.md) — canonical term registry
- [Input grammar](reference/input-grammar.md) — `@file` `/skill` `./path`
- [Keyboard shortcuts](reference/keyboard.md) — every shortcut by scope
- Tool cards: [bash](reference/tools/bash.md) · [patch](reference/tools/patch.md) · [keyword_search](reference/tools/keyword_search.md) · [browser](reference/tools/browse.md) · [propose_task](reference/tools/propose_task.md) · [spawn_agents](reference/tools/spawn_agents.md) · [ask_user_question](reference/tools/ask_user_question.md) · [skill](reference/tools/skill.md) · [tmux](reference/tools/tmux.md)
- [Modes matrix](reference/modes-matrix.md) — capability grid per mode
- [Conversation states](reference/conversation-states.md) — busy vs. waiting
- [Command palette](reference/command-palette.md) — `Ctrl/Cmd+P`

---

*Contributing to this guide?* Read [`AUTHORING.md`](AUTHORING.md) for the
principles and pre-flight checklist, start from a template in
[`_templates/`](_templates/), and keep [`SUMMARY.md`](SUMMARY.md) in sync. The
`phoenix-guide-sync` skill audits this guide against the code and specs on a
schedule.
