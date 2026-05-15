# Prevent draft text loss from UI insert/restore actions

## Problem

Several UI actions can overwrite the message composer draft without checking whether the user already has unsent text. The bash `peek` action is the worst offender: clicking `peek <handle>` dispatches `phoenix:insert-draft`, and `InputArea` handles that event by calling `setDraft(text)`, replacing the user's current draft. That bash peek action is not a desired user-facing workflow and should be removed from the rendering entirely.

The audit found these risky code paths:

- `ui/src/components/InputArea.tsx`
  - `phoenix:insert-draft` event handler calls `setDraft(text)` unconditionally.
  - Imperative `setDraft` handle exposed to parent components also replaces unconditionally.
- `ui/src/components/MessageComponents.tsx`
  - Running bash result `peek` button dispatches `phoenix:insert-draft` with `peek bash handle <handle>`.
- `ui/src/components/SkillViewer.tsx`
  - “Insert /skill into input” dispatches `phoenix:insert-draft`, which currently overwrites the draft.
- `ui/src/pages/ConversationPage.tsx`
  - Seed-draft hydration calls `inputRef.current?.setDraft(draft)` after mount; safe only for a fresh seeded conversation, but should not clobber if the user types before the timeout fires.
  - Failed-message retry calls `inputRef.current?.setDraft(msg.text)` after dismissing the failed queued message, which can clobber a current draft.
- `ui/src/pages/ChainPage.tsx`
  - Chain re-ask calls `DRAFT_SET` with the previous question, overwriting any current chain draft.
  - Chain localStorage hydration calls `DRAFT_SET(saved)` if saved exists; likely safe on mount, but should avoid overwriting a non-empty draft.

## Desired behavior

Draft-preserving actions should append, insert at cursor, or ask for confirmation instead of silently replacing existing text.

Recommended policy:

1. Introduce a single composer operation API rather than generic replacement events:
   - `appendToDraft(text)` for contextual inserts.
   - `replaceDraft(text, { allowOverwrite?: boolean })` only for explicit restore flows.
   - External `phoenix:insert-draft` should default to append-or-cursor-insert, not replace.
2. Remove the bash `peek` action from running bash result rendering.
   - Do not replace it with another draft-mutating affordance.
   - Users who want fresh process output can ask explicitly in the normal message composer.
3. Failed-message retry and chain re-ask should preserve existing draft text.
   - If draft is empty, populate it.
   - If draft is non-empty, append with spacing or prompt/confirm before replace.
4. Hydration flows should be guarded.
   - Only apply stored seed/chain drafts if the current draft is still empty.
5. Add tests for the overwrite cases.

## Acceptance criteria

- Running bash results no longer render a `peek` button/action.
- Inserting a skill from `SkillViewer` cannot erase existing message input text.
- Retrying a failed message cannot silently erase an unrelated current draft.
- Chain re-ask cannot silently erase an unrelated current chain draft.
- Seed/chain hydration does not overwrite text typed before hydration applies.
- Tests cover at least the shared `phoenix:insert-draft` behavior and one parent-driven restore/reask path.
