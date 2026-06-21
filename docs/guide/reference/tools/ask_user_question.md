---
title: ask_user_question
summary: The agent pauses to ask 1–4 multiple-choice questions (2–4 options each), optionally with previews.
category: reference
keywords: [ask_user_question, question, choice, preview, pause]
related: [concepts/conversations.md, reference/glossary.md]
---

# ask_user_question

> **At a glance:** when the agent needs a decision it can't make alone, it pauses
> and asks **1–4** structured questions, each with **2–4** options.

## What it does

Presents a question panel; you pick an option per question (or choose the
always-present **Other** for free text), optionally annotating your choice. The
agent resumes with your answers.

## Parameters & limits

| Field | Bound |
|-------|-------|
| questions | 1–4 |
| options per question | 2–4 |
| `multiSelect` | pick several (per question) |
| `preview` | optional per-option preview — **single-select only** |

## What you'll see

Execution **pauses** at a question panel until you answer. Single-select
questions with previews render side-by-side. Dismissing the panel without
answering tells the agent *nothing* — send a message to resume.

## Limits & gotchas

- Previews are dropped when `multiSelect` is on.
- Parent conversations only — sub-agents can't ask (they run autonomously).
- The agent can't author the "Other" option; Phoenix always adds it.

## Related

- [Conversations](../../concepts/conversations.md) — the paused state
- [Glossary](../glossary.md) — canonical terms
