# Add AI-generated rename to single-conversation rename dialog

## Goal

Add a “Generate with AI” affordance to the existing single-conversation rename editor so a user can repair deterministic/random slugs after initial title generation failed (quota, VPN, temporary provider outage, etc.).

## Current context

- Manual conversation rename already exists:
  - UI: `RenameDialog` used by `Sidebar` and `ConversationListPage`.
  - Client API: `api.renameConversation(convId, name)`.
  - Backend: `POST /api/conversations/:id/rename` → `db.rename_conversation`.
- Chain AI rename recently landed:
  - Backend: `POST /api/chains/:rootId/regenerate-name`.
  - Client API: `api.regenerateChainName(rootId)`.
  - UI: `ChainPageHeader` button with in-flight state and failure handling.
- Existing title generation for new conversations already produces slug-shaped titles via `title_generator::generate_title` and `sanitize_title`.
- DB has `first_opening_message_text(conv_id)`, already used by chain rename, which is suitable for single-conversation regeneration.

## Plan

1. **Backend endpoint**
   - Add `POST /api/conversations/:id/regenerate-name` (or `/regenerate-slug` if preferred, but keep naming parallel with chain `regenerate-name`).
   - Handler flow:
     1. Load first opening message via `db.first_opening_message_text(&id)`.
     2. If no usable opening message, return an error and leave the slug unchanged.
     3. Require a cheap model from the model registry.
     4. Call `title_generator::generate_title(&opening, cheap_model)`.
     5. If generation returns `None` or empty, return an error and leave the slug unchanged.
     6. Persist using the existing `db.rename_conversation(&id, &generated_slug)` path so duplicate-slug handling and `updated_at` behavior stay centralized.
     7. Return the refreshed conversation response, same shape as manual rename.
   - Map duplicate slug to a clear user-facing error. If collisions are likely enough to matter, add a small suffix retry loop rather than failing the generation.

2. **TypeScript API client**
   - Add `api.regenerateConversationName(convId)` (or `regenerateConversationSlug`) that POSTs to the new endpoint and returns the refreshed conversation response.

3. **Rename dialog UI**
   - Extend `RenameDialog` with optional generation props, e.g.:
     - `onGenerate?: () => Promise<string | void>` or `onGenerate?: () => Promise<{ slug: string }>`
     - `generatingLabel` / fixed button text “Generate with AI”.
   - Add a secondary “Generate with AI” button inside the dialog.
   - While generation is in flight:
     - disable manual submit and generate button,
     - show “Generating…” or a small spinner,
     - keep Cancel available if practical, unless abort support is intentionally omitted.
   - On success:
     - update the input to the generated slug,
     - close the dialog if the backend already persisted the rename, matching chain behavior; or alternatively keep the dialog open with the generated value prefilled if product preference is to preview before accepting. Prefer closing for parity with chain regeneration and because the action says “Generate with AI,” not “Suggest.”
   - On failure:
     - surface the error in the existing dialog error area,
     - leave the current input/slug unchanged.

4. **Integrate both single-conversation rename callers**
   - `Sidebar`: pass `onGenerate` to `RenameDialog`, refresh conversation list after success, and navigate/update active route if the renamed conversation is currently open so the URL does not keep the stale slug.
   - `ConversationListPage`: pass the same generation handler and refresh after success.
   - Preserve existing manual rename behavior.

5. **Tests**
   - Backend tests for the new handler/regeneration behavior:
     - no opening message leaves slug unchanged and returns an error,
     - LLM failure leaves slug unchanged,
     - successful generation uses the existing rename path and returns refreshed conversation data,
     - duplicate slug maps cleanly (or suffix-retries, depending on implementation choice).
   - UI tests for `RenameDialog`:
     - renders “Generate with AI” only when generation prop is supplied,
     - disables appropriate buttons and shows in-flight state,
     - success triggers parent refresh/close behavior,
     - failure shows the error without closing.
   - Caller tests for `Sidebar` / `ConversationListPage` as feasible, especially the active-conversation URL update after slug change.

## Acceptance criteria

- From the existing conversation rename dialog, a user can click “Generate with AI” to replace a poor deterministic/random slug with an LLM-generated slug.
- Manual rename still works exactly as before.
- Generation failures never overwrite the existing slug.
- Successful generation persists through the existing conversation rename semantics and refreshes visible conversation lists.
- If the active conversation is renamed from the sidebar, the browser route is updated from `/c/<old>` to `/c/<new>`.
- Covered by backend and UI tests.

## Validation

Run:

```bash
./dev.py check
```

If iterating narrowly first:

```bash
cargo test regenerate_conversation
cd ui && pnpm test -- RenameDialog Sidebar ConversationListPage
```
