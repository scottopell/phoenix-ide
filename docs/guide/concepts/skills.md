---
title: Skills
summary: Reusable instruction sets — SKILL.md files — that you or the agent invoke by name to load a workflow into the conversation.
category: concepts
keywords: [skill, instructions, workflow, invoke, discovery, arguments]
related: [howto/compose-with-references.md, reference/tools/skill.md, reference/glossary.md]
---

# Skills

A **skill** is a reusable set of instructions — a `SKILL.md` file — that you or
the agent can invoke by name to load a workflow, standard, or procedure into the
conversation. Skills are how a repeatable "here's how we do X" becomes a single
call instead of a re-typed paragraph.

```
 /skill-name   (you, in the composer)  ─┐
                                         ├─▶ the same instructions load into context
 skill tool    (the agent)             ─┘
```

## How they work

- **One skill, two callers.** You invoke `/skill-name`; the agent invokes the
  `skill` tool. Both load the *same* expanded instructions.
- **Discovered, closest wins.** Skills live in `.claude/skills/` and
  `.agents/skills/` from the working directory up to root (plus your home dir).
  A skill nearer your project shadows one further out.
- **Parameterized.** A skill can take arguments, substituted into its body.
- **Catalog, not bulk.** The agent always sees the *list* of skill names and
  descriptions; it loads a skill's full body only when it's invoked.

## What you'll see

A **Skills** panel lists what's available, grouped (built-in, per-project, your
own). Open one to read it, or drop it into your message — see
[Compose with references](../howto/compose-with-references.md).

> **Remember:** a `/skill` you type and the agent's `skill` tool load *identical*
> instructions — skills are shared between your direction and the agent's
> autonomous work, not two separate systems.

## See also

- [skill](../reference/tools/skill.md) — the tool reference (invocation, arguments)
- [Compose with references](../howto/compose-with-references.md) — `/skill` in the composer
- [Glossary](../reference/glossary.md) — skill
