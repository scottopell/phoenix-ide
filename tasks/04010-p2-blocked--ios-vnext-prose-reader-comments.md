# Add the iOS vNext prose reader and commenting interface

## Outcome

Satisfy `REQ-IOS-020` and `REQ-IOS-021` with a focused Markdown/prose reading surface and understandable session-scoped review notes.

## Dependencies

Blocked by ProductConversation migration, Markdown rendering, grounding/files, and the rendering fixture harness.

## Scope

Catalog reader navigation, readable typography, line/block anchoring, live-target and content-revision eligibility, in-memory notes, Send-to-composer formatting, typed draft authority, submit-time revalidation, discard confirmation, and return-to-conversation flow. Then create numbered requirement-backed leaf tasks for independently reviewable reader and commenting behaviors. Reuse `REQ-PF-009` through `REQ-PF-011` and the ordinary durable message queue rather than creating a second delivery lifecycle. Each component leaf task adds its deterministic fixtures to the base harness.

## Acceptance

- Supported server file contents open in a readable native surface.
- Comment anchors remain understandable across supported Markdown structures.
- Notes bind to the exact live-target `WorkScope`, file identity, and current content revision; other scopes, stale content, and conversations that reject chat remain read-only.
- A target or revision change keeps in-session notes visible but disables Send until refresh/re-anchor or explicit discard.
- Send formats notes into the editable conversation composer, attaches typed authority for the exact scope/file/revision bindings, clears the reader notes, and closes the reader.
- Composer edits preserve review authority; actual message submission revalidates every binding and clears authority only after durable queue acceptance.
- Failed submit-time revalidation preserves the draft and its authority, then offers refresh/re-anchor or removal of the affected review contribution without losing unrelated draft text.
- Closing with unsent notes requires Cancel or explicit Discard; reopening starts with zero notes.
- The leaf queue separates reader fidelity from session-scoped note composition.

## Out of scope

Direct comment delivery, durable comment persistence, general file editing, and code-review diff annotation.
