---
title: Compose with references
summary: Pull a file's contents, a skill, or a path into a message with the composer's @ / ./ prefixes.
category: howto
keywords: [compose, reference, "@file", "/skill", "./path", include, autocomplete]
related: [reference/input-grammar.md, reference/glossary.md]
---

# Compose with references

The message composer recognizes three prefixes that pull context into your
message, each with autocomplete. Use them instead of pasting file contents or
re-typing paths.

## Before you start

You're in the message composer of any conversation.

## Steps

1. **Include a file's contents.** Type `@` and start the path, then pick from the
   dropdown (`Tab` to accept). At send, the file's *contents* are handed to the
   agent; your message still shows the `@path`.
2. **Load a skill.** Type `/` at the start of the message (or after a space) and
   the skill name. Add trailing text to pass as the skill's arguments. The
   skill's instructions load into the agent's context.
3. **Reference a path without including it.** Type `./` and the path. Only the
   literal path is sent — the agent reads it if it needs to. Reach for this with
   large files you don't want inlined.

## Result

Your message carries exactly the context you named: file contents from `@`, skill
instructions from `/`, or a path pointer from `./` — visible as the original
reference in history, expanded only for the agent.

## Troubleshooting

- **Send is blocked with an inline error.** An `@` or `/` reference didn't
  resolve (missing file, binary file, or unknown skill) — fix or remove it.
  A `./path` never blocks. Exact error text and limits are in
  [Input grammar](../reference/input-grammar.md#when-a-reference-cant-resolve).
- **The dropdown didn't open.** `/` only triggers at the start of the message or
  after a space; `@` and `./` trigger anywhere.

## See also

- [Input grammar](../reference/input-grammar.md) — the dense reference
- [Glossary](../reference/glossary.md) — canonical terms
