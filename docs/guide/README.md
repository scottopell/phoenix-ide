---
title: Phoenix User Guide
summary: Learn to drive Phoenix — conversations, modes, tasks, sub-agents, and the tools the agent uses on your behalf.
category: landing
keywords: [guide, help, getting started, overview]
related: [concepts/overview.md, howto/getting-started.md]
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

> This page and every page under it render both on GitHub and inside Phoenix
> (`?` for the quick overlay, or open the full Help page from the command
> palette). The [`SUMMARY.md`](SUMMARY.md) manifest is the table of contents
> both renderers read.

## Concepts

- [Overview](concepts/overview.md) — *(planned)*
- [Conversations](concepts/conversations.md) — *(planned)*
- [Modes](concepts/modes.md) — Direct, Explore, Work, Branch
- [Tasks](concepts/tasks.md) — *(planned)*
- [Sub-agents](concepts/sub-agents.md) — *(planned)*
- [Skills](concepts/skills.md) — *(planned)*
- [Chains](concepts/chains.md) — *(planned)*
- [Workspace](concepts/workspace.md) — worktrees & resource ownership *(planned)*
- [Permissions](concepts/permissions.md) — *(planned)*
- [Providers & models](concepts/providers.md) — *(planned)*

## How-to

- [Getting started](howto/getting-started.md) — *(planned)*
- [Run a managed task](howto/run-a-managed-task.md) — Explore → approve → Work → PR
- [Spawn sub-agents](howto/spawn-sub-agents.md) — *(planned)*
- [Use the terminal](howto/use-the-terminal.md) — *(planned)*
- [Review changes](howto/review-changes.md) — *(planned)*
- [Share read-only](howto/share-read-only.md) — *(planned)*
- [Steer a running agent](howto/steer-a-running-agent.md) — *(planned)*
- [Compose with references](howto/compose-with-references.md) — `@file` `/skill` `./path` *(planned)*
- [Search conversations](howto/search-conversations.md) — *(planned)*
- [Voice input](howto/voice-input.md) — *(planned)*

## Reference

- [Tools](reference/tools/) — one card per tool the agent can use
  - [bash](reference/tools/bash.md)
  - others — *(planned)*
- [Keyboard shortcuts](reference/keyboard.md) — *(planned)*
- [Input grammar](reference/input-grammar.md) — `@` `/` `./` *(planned)*
- [Modes matrix](reference/modes-matrix.md) — mode × tool availability *(planned)*
- [Conversation states](reference/states.md) — *(planned)*
- [Command palette](reference/command-palette.md) — *(planned)*
- [Glossary](reference/glossary.md) — *(planned)*

---

*Contributing to this guide?* Use the templates in [`_templates/`](_templates/)
and keep [`SUMMARY.md`](SUMMARY.md) in sync. The `phoenix-guide-sync` skill
audits this guide against the code and specs on a schedule.
