---
title: Permissions
summary: A deny layer that checks every consequential tool call before it runs — unsafe actions are blocked by construction, not by asking.
category: concepts
keywords: [permissions, deny, safety, gate, denial, escalation]
related: [reference/tools/bash.md, concepts/modes.md, reference/glossary.md]
---

# Permissions

**Permissions** are a deny layer that inspects every consequential tool call
*before* it runs. Unsafe actions are blocked structurally — the agent can't route
around the check — rather than merely being discouraged.

```
 agent wants to run a tool ──▶ [ deny check ] ──▶ runs
                                     │
                                     └─ denied → the agent is told why, and tries another way
```

## How it works

- **Intent-agnostic.** The check judges only the tool name and its input — not
  the conversation, your messages, or prior results. The same call is allowed or
  denied the same way every time.
- **Typed rules per tool.** It's seeded with shell-safety rules: blind
  `git add` (`-A`/`.`/`--all`/`*`), force-push (`--force`/`-f`, but
  `--force-with-lease` is fine), and dangerous `rm -rf` of `/`, `~`, `$HOME`,
  `.git`, `*`, and `.*`.
- **Deny and continue.** A denial comes back to the agent through the normal
  tool-result channel, so it can adapt and try a safe alternative.

## What you'll see

A denied call appears in the transcript as a tool result explaining what was
blocked and why; the agent keeps going. There's no modal to click through — the
gate is automatic.

> **Remember:** the gate is correct-by-construction — a tool call that hasn't
> passed it can't reach execution at all. Denial happens *before* the action,
> never after.

## See also

- [bash](../reference/tools/bash.md) — the commands the safety rules screen
- [Modes](../concepts/modes.md) — the other layer that bounds what the agent can do
- [Glossary](../reference/glossary.md) — permissions
