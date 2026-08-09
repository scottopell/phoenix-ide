# Add the iOS vNext prose reader and commenting interface

## Outcome

Satisfy `REQ-IOS-020` and `REQ-IOS-021` with a focused Markdown/prose reading surface and durable, understandable review comments.

## Dependencies

Blocked by ProductConversation migration, Markdown rendering, grounding/files, and the rendering fixture harness.

## Scope

Catalog reader navigation, readable typography, line/block anchoring, draft notes, send/retry/discard behavior, and return-to-conversation flow. Then create numbered requirement-backed leaf tasks for independently reviewable reader and commenting behaviors. Each component leaf task adds its deterministic fixtures to the base harness.

## Acceptance

- Supported server file contents open in a readable native surface.
- Comment anchors remain understandable across supported Markdown structures.
- Draft comments survive transient send failures and cannot disappear silently.
- Leaving with unsent notes requires successful send or explicit discard.
- The queue separates reader fidelity from comment lifecycle work.

## Out of scope

General file editing and code-review diff annotation.
