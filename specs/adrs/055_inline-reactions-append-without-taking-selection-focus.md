# ADR-055: Inline reactions append without taking selection focus

- **Status:** Accepted
- **Date:** 2026-09-19
- **Affects:** REQ-PF-018, REQ-PF-019, REQ-PF-020, REQ-PF-021, REQ-KB-001, REQ-KB-004

## Context

The user frequently reviews assistant answers by opening the message viewer and batching line annotations into a draft. They want to react to selected passages directly in the transcript, immediately after selection, while keeping the existing reviewers. Native copy menus and mobile selection handles must remain useful.

## Options considered

1. Open and focus a reaction editor on selection. Fast typing, but steals native selection focus and raises the mobile keyboard before the user asks to type.
2. Show an activation icon, then open the editor on a second action. Preserves selection but adds a step to every reaction; the user explicitly prefers immediate input visibility.
3. Immediately reveal an unfocused input and append each completed reaction into the ordinary draft. Keeps selection native and makes typing an explicit action.

## Decision

Choose the third option. A selected-text reaction is a non-modal exception to automatic panel focus. Use ListPlus and Add to draft to distinguish the action from clipboard copying and message submission. Keep source identity and the complete selected quote independent of the live DOM selection. Use a delimited text block so selected code and Markdown cannot terminate the quotation.

Retain one unfinished typed reaction per conversation in an in-memory store owned by the application provider, so source virtualization and in-app navigation do not silently destroy a thought. Appending goes through the existing draft store without the reviewer's close/focus side effects. No new persistence or server protocol is introduced. Existing batch review remains a separate interaction.

## Consequences

- Native selection and copying remain available; selecting alone does not summon the keyboard.
- The user needs to click or tap the visible input to type.
- A typed reaction stays pinned until added or explicitly discarded; a second selection does not silently retarget it.
- Unfinished reactions survive in-app navigation but not browser reload or app termination. Appended text inherits ordinary draft persistence.
- Mobile system menus cannot be controlled by web layout. Real-device qualification is necessary in addition to responsive browser tests.

## References

- `InlineMessageReaction`, `InlineReactionStore`, `DraftStore`, `MessageReviewAction`
- `specs/prose-feedback/requirements.md`
- `specs/keyboard-interaction/requirements.md`
