---
status: ready
priority: p2
created: 2025-02-08
---

# HTML Entity Encoding Bug in Message Display

## Problem

When a user sends a message containing quotes, they are displayed as HTML entities (`&quot;`) instead of actual quote characters.

## Reproduction

1. Go to `/new`
2. Type a message like: `Say "Hello" please`
3. Send the message
4. Observe the user message displays as: `Say &quot;Hello&quot; please`

## Expected

Quotes should render as actual quote characters, not HTML entities.

## Location

Likely in the message rendering code in `ui/src/components/MessageBubble.tsx` or wherever user messages are displayed.

## Screenshots

See QA test at commit 0a5cd69 - the message "Say &quot;Hello QA test&quot; and nothing else" shows HTML entities.
