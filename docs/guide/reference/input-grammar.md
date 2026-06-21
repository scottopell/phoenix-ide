---
title: Input grammar
summary: The composer's three reference prefixes — @file, /skill, ./path — with autocomplete, error, and limit details.
category: reference
keywords: [input, grammar, reference, "@file", "/skill", "./path", autocomplete, expansion]
related: [howto/compose-with-references.md, reference/glossary.md]
---

# Input grammar

> **At a glance:** three prefixes in the message composer — `@file` includes a
> file's contents, `/skill` loads a skill, `./path` inserts a literal path. Type
> a prefix to open autocomplete; `@` and `/` are expanded at send, `./` is not.

## The three forms

| Type | Opens autocomplete on | Sent to the agent | Blocks send if unresolved? |
|------|-----------------------|-------------------|----------------------------|
| `@file` | `@` anywhere | the file's **contents**, injected as a `<file path="…">` block | **yes** |
| `/skill` | `/` at message start or after whitespace | the skill's instructions (with your trailing text as `$ARGUMENTS`) | **yes** |
| `./path` | `./` anywhere | the **literal path** only — the agent decides what to read | no — never validated |

What history shows is the original `@…` / `/…` / `./…`; the expansion (file
contents, skill instructions) goes only to the agent. Use `./path` for large
files you don't want inlined.

## Autocomplete

A dropdown opens with the prefix and a mode hint — `file will be included`,
`skill invocation`, or `path reference`. Then:

| Key | Action |
|-----|--------|
| `↑` / `↓` | move the selection |
| `Tab` | accept the highlighted item |
| `Enter` | accept if the dropdown is open, otherwise send |
| `Esc` | dismiss without inserting |

A skill that declares an argument hint shows it as ghost text once selected.

## When a reference can't resolve

An unresolvable `@` or `/` blocks send with an inline error; a `./path` never
blocks (it's never expanded). Editing the draft clears the error.

| Error (verbatim) | Cause |
|------------------|-------|
| `File not found: {path}` | `@` target missing |
| `File is binary and cannot be included: {path}` | `@` target isn't UTF-8 text |
| `Skill '{name}' failed: {error}` | `/` skill missing or errored |

## Limits

- Autocomplete lists up to **50** matches.
- `@file` inlines the **entire** file (UTF-8 text only; no size cap) at send time.
- File *attachments* (a separate feature, not a reference): **10 MB** per file,
  **10** files, **25 MB** total.

## Related

- [Compose with references](../howto/compose-with-references.md) — the how-to
- [Glossary](glossary.md) — canonical terms
