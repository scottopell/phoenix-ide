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

> This page and every page under it render both on GitHub and inside Phoenix
> (`?` for the quick overlay, or open the full Help page from the command
> palette). The [`SUMMARY.md`](SUMMARY.md) manifest is the table of contents
> both renderers read.

Linked entries are written; the rest are `*(planned)*` and listed in
[`SUMMARY.md`](SUMMARY.md). (Planned pages aren't linked here — a link to an
unwritten page would 404 on GitHub.)

## Concepts

- [Modes](concepts/modes.md) — Direct, Explore, Work, Branch
- [Tasks](concepts/tasks.md) — the plan that gates Explore → Work
- [Chains](concepts/chains.md) — continuation runs, queryable as a unit
- Overview, Conversations, Sub-agents, Skills, Workspace, Permissions, Providers — *(planned)*

## How-to

- [Run a managed task](howto/run-a-managed-task.md) — Explore → approve → Work → PR
- Getting started, Spawn sub-agents, Use the terminal, Review changes, Share read-only, Steer a running agent, Compose with references, Search conversations, Voice input — *(planned)*

## Reference

- [Managed lifecycle states](reference/managed-lifecycle-states.md) — approval & Done? bar states
- [Glossary](reference/glossary.md) — canonical term registry
- [bash](reference/tools/bash.md) — and more tool cards *(planned)*
- Keyboard shortcuts, Input grammar, Modes matrix, Conversation states, Command palette — *(planned)*

---

*Contributing to this guide?* Read [`AUTHORING.md`](AUTHORING.md) for the
principles and pre-flight checklist, start from a template in
[`_templates/`](_templates/), and keep [`SUMMARY.md`](SUMMARY.md) in sync. The
`phoenix-guide-sync` skill audits this guide against the code and specs on a
schedule.
