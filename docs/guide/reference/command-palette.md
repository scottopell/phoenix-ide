---
title: Command palette
summary: Ctrl/Cmd+P — search conversations and files, or run a command. Desktop only.
category: reference
keywords: [command palette, search, actions, "Ctrl+P", "Cmd+P"]
related: [howto/search-conversations.md, reference/keyboard.md, reference/glossary.md]
---

# Command palette

> **At a glance:** press `Ctrl/Cmd+P` to open it. Type to **search** conversations
> (and files); type `>` first to run an **action**. Desktop only.

## Modes

| Mode | Enter by | Input reads |
|------|----------|-------------|
| Search *(default)* | just type | `Search conversations…` — or `Search conversations and files…` inside a conversation |
| Action | type `>` | `Type a command…` |

In search mode, results group by source (conversations show slug + directory).
`Backspace` on an empty action input drops back to search.

## Built-in actions

| Action (verbatim) | Category | Does |
|-------------------|----------|------|
| `New Conversation` | Conversation | go to the conversation list (`/`), where **+ New** starts one |
| `Go to Conversation List` | Navigation | open the conversation list (`/`) |
| `Open User Guide` | Help | open `/help` |
| `Archive Current Conversation` | Conversation | archive the open conversation — or, if it belongs to a **chain**, the whole chain *(only when a conversation is open)* |

## Keyboard

| Key | Action |
|-----|--------|
| `↑` | previous result |
| `↓` / `Ctrl+N` | next result |
| `Enter` | select / run |
| `Esc` | close |

(`Ctrl/Cmd+P` toggles the palette — pressing it while open **closes** it, so use
`↑` to move up.)

## Related

- [Search conversations](../howto/search-conversations.md) — the search how-to
- [Keyboard shortcuts](keyboard.md) — all shortcuts
- [Glossary](glossary.md) — canonical terms
