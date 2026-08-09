# Complete iOS vNext Markdown rendering

## Outcome

Render conversation Markdown as readable native content rather than inline-only attributed text.

## Dependencies

Blocked by ProductConversation migration and the rendering fixture harness.

## Scope

Catalog the Markdown grammar Phoenix emits, then create numbered leaf tasks for bounded renderer families such as fenced code, tables, lists, links, images, block quotes, and streaming/finalized consistency. Reuse shared parsing policy where possible without coupling the client to web DOM behavior.

## Acceptance

- Common Phoenix Markdown structures are legible and stable on phone and tablet widths.
- Streaming and finalized forms do not materially disagree.
- Unsupported or malformed input degrades visibly without losing source text.
- The queue is divided into independently reviewable renderer tasks.

## Out of scope

File browsing and line-level prose comments.
