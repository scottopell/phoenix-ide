# Add the iOS vNext prose reader and commenting interface

## Outcome

Satisfy `REQ-IOS-020` and `REQ-IOS-021` with a focused Markdown/prose reading surface and understandable session-scoped review notes.

## Dependencies

Blocked by ProductConversation migration, Markdown rendering, grounding/files, and the rendering fixture harness.

## Scope

Catalog reader navigation, readable typography, line/block anchoring, in-memory notes, Send-to-composer formatting, discard confirmation, and return-to-conversation flow. Then create numbered requirement-backed leaf tasks for independently reviewable reader and commenting behaviors. Reuse `REQ-PF-009` through `REQ-PF-011` rather than creating a second delivery lifecycle. Each component leaf task adds its deterministic fixtures to the base harness.

## Acceptance

- Supported server file contents open in a readable native surface.
- Comment anchors remain understandable across supported Markdown structures.
- Send formats notes into the editable conversation composer, clears them, and closes the reader.
- Closing with unsent notes requires Cancel or explicit Discard; reopening starts with zero notes.
- The leaf queue separates reader fidelity from session-scoped note composition.

## Out of scope

Direct comment delivery, durable comment persistence, general file editing, and code-review diff annotation.
