---
title: skill
summary: Load a reusable instruction set by name — /skill for you, the skill tool for the agent — with optional arguments.
category: reference
keywords: [skill, "/skill", instructions, arguments, invoke]
related: [concepts/skills.md, howto/compose-with-references.md, reference/glossary.md]
---

# skill

> **At a glance:** invoke a [skill](../../concepts/skills.md) — a `SKILL.md` of
> procedural instructions — by name. You type `/name`; the agent calls the `skill`
> tool. Either way the skill's instructions load into context.

## What it does

Expands a named skill: strips its frontmatter, prepends its base directory (so it
can point at companion files), substitutes any arguments, and delivers the body.

## Invocation & arguments

| Caller | Form |
|--------|------|
| You | `/name [args]` in the composer |
| Agent | the `skill` tool with `name` + optional `arguments` |

Arguments are whitespace tokens: `$ARGUMENTS` (all), `$1`, `$2`, … (positional).
If a skill has no `$ARGUMENTS` placeholder, your args are appended.

## What you'll see

Your `/name` stays in history; the expanded instructions go to the agent. The
skill catalog (names + descriptions) is always visible to the agent; bodies load
on demand.

## Limits & gotchas

- **Discovery:** `.claude/skills/` and `.agents/skills/` from the working dir up
  to root (plus `$HOME`); the **closest** definition wins.
- An unknown skill name errors.

## Related

- [Skills](../../concepts/skills.md) — the concept
- [Compose with references](../../howto/compose-with-references.md) — `/skill` in the composer
- [Glossary](../glossary.md) — canonical terms
