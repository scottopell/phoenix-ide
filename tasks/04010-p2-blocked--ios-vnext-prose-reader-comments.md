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
- A note that cannot be re-anchored can be discarded individually while other reader notes remain intact.
- Send formats notes into a structural editable conversation contribution that owns both text and exact scope/file/revision authority, then clears the reader notes and closes the reader.
- Composer edits preserve each retained review contribution's authority; removal deletes its text and authority together without disturbing unrelated text or images.
- Independently editable plain segments can be inserted before, between, or after review contributions.
- Actual message submission revalidates every binding, preserves staged images in the same outbox entry, and clears the draft only after positive disk-persistence evidence.
- The ordinary queue entry cannot become sendable until persistence evidence exists; a cleared composer is reinitialized with an editable empty plain segment.
- Persistence-pending review messages remain optimistically visible, structurally non-sendable, and retryable on later delivery triggers.
- Connectivity, valid init, foreground, and turn-completion triggers retry pending persistence; every retry and receipt conversion revalidates review authority.
- Authority-evidence changes immediately revalidate pending persistence; invalid authority unlocks the draft with typed failures without waiting for another delivery trigger.
- Invalid authority and hard deletion retain a durable, non-sendable invalidation tombstone across retry and restart until version-fenced removal is acknowledged; positive ordinary-entry creation after valid persistence enters the persistence-fenced drain in durable oldest-first order.
- POST attempts have typed in-flight ownership that is released on success, definitive rejection, or transport interruption before automatic redelivery can select the entry again.
- Failed submit-time revalidation preserves the draft and emits typed failures per affected contribution, then clears stale failures when authority is repaired or removed.
- Closing with unsent notes requires Cancel or explicit Discard; reopening starts with zero notes.
- Hard-delete/not-found cleanup removes drafts, pending renders, and authority while retaining an acknowledged-removal tombstone until every durable pending review payload is fenced out.
- The leaf queue separates reader fidelity from session-scoped note composition.

## Out of scope

Direct comment delivery, durable comment persistence, general file editing, and code-review diff annotation.
