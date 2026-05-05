---
created: 2026-05-05
priority: p2
status: ready
artifact: ui/src/components/ConversationList.tsx
---

# Chain-atomic lifecycle: UI

## Context

The backend (commit `b93bb792` on branch `feat/chain-atomic-lifecycle`) now treats
chains as atomic units for archive/delete. Per-conversation `archive`,
`unarchive`, `delete` endpoints return **409 with `error_type: "chain_member"`**
when the target is part of a chain (≥2 members), with `conflict_slug` pointing
at the root. Three new endpoints handle the whole-chain operations:

- `POST /api/chains/:rootId/archive`
- `POST /api/chains/:rootId/unarchive`
- `DELETE /api/chains/:rootId`

This task is the UI work that surfaces the new model.

## Principle

Chains are first-class: archive/delete act on the whole chain, never on a
single member. The UI must make the chain-level action the obvious path and
remove the per-member affordances that would now produce 409s.

## Settled design decisions

| Decision | Outcome |
|---|---|
| Where do chain-level archive/delete actions live? | Sidebar chain header `⋮` menu + ChainPage header buttons. Member rows lose Archive/Delete (Rename only). |
| Delete confirm depth | Scope-explicit: name, member count + IDs, worktree count, "cannot be undone". |
| Archived-list rendering | Archived chains group the same as active. Chain header `⋮` shows Unarchive / Delete. Standalone archived conversations render flat alongside the chain blocks. |
| Per-member rename | Unchanged. Slugs stay per-conversation; only the lifecycle ops promote to chain-scope. |
| Worktree count for the confirm | Add a typed `has_worktree: bool` field to `ChainMemberSummary` on the Rust side. Avoids loading every full conversation client-side. |

## Implementation plan

### 1. Backend: add `has_worktree` to `ChainMemberSummary`

- File: `src/api/chains.rs` — add `pub has_worktree: bool` to `ChainMemberSummary`.
- Populate in `build_member_summaries` (and the test helper `build_view_for_test`).
  Rule: `true` when `conv.conv_mode` is `Work { .. }` or `Branch { .. }`, else `false`.
- The struct already derives `ts_rs::TS` with `#[ts(export)]`; running `./dev.py codegen`
  (or `cargo test`) will regenerate `ui/src/generated/ChainMemberSummary.ts`.
- Add a unit test asserting `has_worktree` reflects the conversation's mode.

### 2. UI: API client (`ui/src/api.ts`)

Add three exports modeled after `archiveConversation` / `deleteConversation`:

```
archiveChain(rootId)   → POST /api/chains/:rootId/archive
unarchiveChain(rootId) → POST /api/chains/:rootId/unarchive
deleteChain(rootId)    → DELETE /api/chains/:rootId
```

Return `Promise<void>` like the per-conv equivalents. Reuse the existing
fetch-error handling pattern (`api.ts` has the precedent — match it).

### 3. Sidebar — `ui/src/components/ConversationList.tsx`

**Chain block header** (`renderChainBlock`):

- Add a `⋮` button at the right edge of `.conv-chain-header`, mirroring the per-row
  pattern (`menuRef`, click-outside dismiss, `expandedId` shape — extend the existing
  state to also track expanded chain id, or add a parallel `expandedChainId`).
- Menu entries:
  - In active list: `Rename chain`, `Archive chain`, `Delete chain`
  - In archived list: `Rename chain`, `Unarchive chain`, `Delete chain`

**Member rows** (`renderConvRow` when `options.isChainMember === true`):

- The dropdown shows `Rename` only.
- Hide the `Archive` / `Unarchive` / `Delete` entries when `isChainMember === true`.
- Standalone rows (where `isChainMember` is undefined / false) keep all three.

**Archived view grouping:**

- The current code says "Archived list stays flat — REQ-CHN-002 scopes chain
  navigation to active conversations." That constraint relaxes now that archive
  is a chain-level op. Run the same `computeChainRoots` / `buildSidebarItems`
  pipeline against `displayList` regardless of `showArchived`.
- The chain page itself (`ChainPage`) only loads chains that exist; archived chains
  are still navigable so the existing chain page works. If the chain page rejects
  archived members for any reason, surface that as a follow-up — don't paper over.

### 4. Page wiring — `ui/src/pages/ConversationListPage.tsx`

- Add handlers `handleArchiveChain`, `handleUnarchiveChain`, `handleDeleteChain`
  paralleling the existing per-conv handlers.
- Pass them down to `ConversationList` as new props
  (`onArchiveChain`, `onUnarchiveChain`, `onDeleteChain`).
- Replace the current single delete-confirm modal (or extend it) so a chain delete
  shows the scope-explicit body. Consider a new `ChainDeleteConfirm` component
  that takes a `ChainView`-shaped object, derives member count + worktree count
  from `members`, and renders the dialog. Keep the per-conv simple confirm for
  standalone conversations.

### 5. ChainPage — `ui/src/pages/ChainPage.tsx`

- Add `Archive` and `Delete` buttons in the header next to the existing rename
  input. Use the same scope-explicit confirm for delete.
- After a successful chain delete, navigate back to the conversation list.
  After archive, navigate back to the active list (chain disappears from active).
- Reuse `archiveChain` / `deleteChain` from `api.ts`.

### 6. Confirm modal — scope-explicit

Render exactly:

```
Delete chain "{display_name}"?

This will permanently remove:
  • {N} conversations (#1, #2, ..., #N)
  • {M} git worktrees     ← only show if M > 0
  • All messages and history

This cannot be undone.

           [Cancel]   [Delete chain]
```

Where:
- `display_name` = `chain.display_name` from the `ChainView`
- N = `members.length`
- The numbered list uses 1-based indices as shown in the sidebar
- M = count of `members` where `has_worktree === true`. If M is 0, omit that bullet.

### 7. Tests

**Vitest**:
- Chain header `⋮` menu renders the right entries in active vs archived modes.
- Member-row dropdown shows only `Rename` when `isChainMember`.
- `ChainDeleteConfirm` body: correct N, list of slugs, worktree count, omits worktree
  bullet when M=0.

**Cargo (Rust side)**:
- New unit test asserting `has_worktree` is `true` for Work and Branch members,
  `false` for Direct/Explore.

## Validation

Run before declaring done:

```
./dev.py check
./dev.py restart  # if any Rust changed
```

Then manually exercise on the running UI:
1. Make a 2+ member chain (continue a context-exhausted conversation)
2. Sidebar chain header `⋮` → Archive chain → all members vanish from active list
3. Archived list → chain block visible → Unarchive chain → reappears in active
4. Sidebar chain header `⋮` → Delete chain → scope-explicit confirm names every
   member and shows worktree count → Delete → all rows gone
5. Member row `⋮` shows Rename only

## Out of scope

- No changes to per-member rename behavior
- No new chain-create UX
- No toast notifications (Phoenix doesn't use them)
- No 409 fallback handler — UI shouldn't fire it after these changes; if a stale
  client does, the existing `conflict_slug` routing handles it via the error path

## QA contract

After implementation, the parent conversation will:
- Verify the visual surfaces match the mockups in this task
- Run `./dev.py check` and confirm all 12 gates pass
- Smoke-test the active and archived flows
- Confirm 409 paths are unreachable through the new UI
