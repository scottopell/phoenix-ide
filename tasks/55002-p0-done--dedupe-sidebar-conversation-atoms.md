# Hotfix duplicated conversations in sidebar

## Problem

Production sidebar can render the same conversation many times. The screenshot shows repeated rows with identical title, created time, mode, and near-identical message counts, implying a single logical conversation is represented by multiple client-side sidebar entries.

Read-only triage points to the client conversation store:

- `useConversationsList()` derives sidebar rows from `store.listSnapshots()`.
- `ConversationStore` is keyed by slug, but logical identity is `Conversation.id`.
- `ConversationStore.upsertSnapshot(row.slug, row)` updates `slugByConvId`, but does not remove an existing atom under a previous/different slug for the same `row.id`.
- Therefore repeated cache/network/SSE observations of the same conversation under varying slug keys can leave duplicate atoms, and `listSnapshots()` returns each atom's `conversation` for rendering.

## Hotfix plan

1. In `ui/src/conversation/ConversationStore.ts`, make `upsertSnapshot` enforce one live atom per `Conversation.id`:
   - Before writing `conversation` under `slug`, look up `existingSlug = slugByConvId.get(conversation.id)`.
   - If `existingSlug` exists and differs from `slug`, remove the old atom only when it belongs to the same `conversation.id`.
   - Preserve the existing monotonic `updated_at` and cached PR merge semantics for the destination atom.
   - Keep `replaceSlugSnapshot` compatible, avoiding double-remove surprises.
2. Add regression tests in `ui/src/conversation/ConversationStore.test.ts`:
   - Upserting the same `id` under a new slug leaves only one snapshot in `listSnapshots()`.
   - The id→slug index points to the newest/current slug.
   - If the old slug has since been reused by a different conversation id, it must not be removed.
3. Add a sidebar-level regression in `ui/src/conversation/useConversationsList.test.tsx` if practical:
   - Seed two slug atoms with the same `Conversation.id` via the public store APIs and assert the rendered active list has one row.
4. Run targeted UI tests:
   - `corepack pnpm --dir ui test -- ConversationStore.test.ts useConversationsList.test.tsx`
5. Run the normal project check before commit if time allows:
   - `./dev.py check`

## Expected result

The sidebar becomes structurally immune to duplicate slug-keyed atoms for the same logical conversation. Even if cache, polling, or SSE observes a slug change or stale alias, only one snapshot per `Conversation.id` remains renderable.

## Urgency

P0 production bug: the sidebar becomes unusable as duplicates accumulate.
