# Add the iOS vNext prose reader and commenting interface

## Outcome

Satisfy `REQ-IOS-020` and `REQ-IOS-021` with a focused Markdown/prose reading surface and understandable session-scoped review notes.

## Dependencies

Blocked by ProductConversation migration, Markdown rendering, grounding/files, and the rendering fixture harness.

## Scope

Catalog reader navigation, readable typography, line/block anchoring, live-target and content-revision eligibility, validated reader re-anchoring, in-memory notes, Send-to-composer formatting, structurally identified draft contributions, attachment preservation, typed submit failures, disk-persistence acknowledgement, discard confirmation, and return-to-conversation flow. Then create numbered requirement-backed leaf tasks for independently reviewable reader and commenting behaviors. Reuse `REQ-PF-009` through `REQ-PF-011` and the ordinary durable message queue rather than creating a second delivery lifecycle. Each component leaf task adds its deterministic fixtures to the base harness.

## Acceptance

- Supported server file contents open in a readable native surface.
- Comment anchors remain understandable across supported Markdown structures.
- Notes bind to the exact live-target `WorkScope`, file identity, and current content revision; other scopes, stale content, and conversations that reject chat remain read-only.
- A target or revision change keeps in-session notes visible but disables Send until refresh/re-anchor or explicit discard.
- Reader re-anchoring validates every retained note against one current revision before updating the session.
- Send formats notes into a structural editable conversation contribution that owns both text and exact scope/file/revision authority, then clears the reader notes and closes the reader.
- Composer edits preserve each retained review contribution's authority; removal deletes its text and authority together without disturbing unrelated text or images.
- Actual message submission revalidates every binding, preserves staged images in the same outbox entry, and clears the draft only after positive disk-persistence evidence.
- Failed submit-time revalidation preserves the draft and emits typed failures per affected contribution, then offers refresh/re-anchor or structural contribution removal.
- Closing with unsent notes requires Cancel or explicit Discard; reopening starts with zero notes.
- The leaf queue separates reader fidelity from session-scoped note composition.

## Out of scope

Direct comment delivery, durable comment persistence, general file editing, and code-review diff annotation.
