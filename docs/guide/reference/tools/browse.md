---
title: browser
summary: Headless-browser automation — navigate, screenshot, click, type, eval, read console, profile — one session per work scope.
category: reference
keywords: [browser, browse, navigate, screenshot, click, eval, console, profile]
related: [concepts/work-scope.md, reference/glossary.md]
---

# browser

> **At a glance:** the agent drives a real headless Chrome — load pages,
> interact, screenshot, read the console/DOM, profile. **One session per
> [work scope](../../concepts/work-scope.md)**, reaped after 30 minutes idle.

## What it does

Launches Chrome on first use and reuses it. The agent navigates, interacts via
real CDP mouse/keyboard events (so framework handlers fire), captures screenshots
and console logs, evaluates JavaScript, and can run performance profiles.

## Operations

These are **separate tools**, each named `browser_*` (there is no single
`browser` tool):

| Group | Tools |
|-------|-------|
| Navigate | `browser_navigate`, `browser_resize` (default viewport 1280×720) |
| Interact | `browser_click`, `browser_type`, `browser_key_press` |
| Observe | `browser_take_screenshot`, `browser_eval`, `browser_recent_console_logs`, `browser_clear_console_logs`, `browser_wait_for_selector` |
| Profile | `browser_profile` (scenarios, CPU/heap/coverage, raw per-run samples) |

## What you'll see

Screenshots inline in the transcript. The **live browser view** can dock beside
the chat — it's read-only (the agent is the sole driver) and shares the side slot
with the diff/prose viewers.

## Limits & gotchas

- **One session per work scope**, shared across a continuation; **30-minute** idle
  timeout (held open while the conversation is live).
- `eval` results and console output over ~4 KB spill to a temp file.
- Browser-native chords (Ctrl+P/W/T/Tab) can't be sent — Chrome intercepts them.

## Related

- [Work scope](../../concepts/work-scope.md) — the session is owned by the work scope
- [Glossary](../glossary.md) — canonical terms
