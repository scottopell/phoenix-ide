# Compact completed chain members in sidebar

## Goal

Make completed, non-latest conversations inside a chain render more compactly in the sidebar. Today they occupy nearly the same visual space as the active/latest chain conversation, differing mostly by the grey terminal dot. The desired result is that older completed chain members are visibly subordinate and denser, while the active/latest member remains easy to identify and continue.

## Proposed UX

For sidebar-mode chain member rows where `getConvDisplayState(conv) === 'terminal'` and the member is not `isChainLatest`:

- Use a compact single-line presentation.
- Keep the position label (`#1`, `#2`, …) and terminal/status dot so the member remains identifiable.
- Hide or greatly de-emphasize verbose metadata such as created/updated time and message count.
- Keep click target, keyboard selection, active selection, and overflow menu behavior intact.
- Do not apply the compact styling to:
  - the latest chain member,
  - the currently active row if that would make active state unclear,
  - standalone completed conversations,
  - the full conversations page unless deliberately supported.

## Implementation sketch

1. Extend `ConversationRow` with a derived class for compact completed chain members, e.g. `conv-item-chain-completed` when:
   - `isChainMember`,
   - `!isChainLatest`,
   - `getConvDisplayState(conv) === 'terminal'`.
2. Add sidebar-scoped CSS under the existing chain block/sidebar rules in `ui/src/index.css`:
   - reduce row padding and margin,
   - keep slug line inline/compact,
   - hide `.conv-item-meta` for compact completed chain members,
   - preserve visible hover, keyboard-selected, active, and menu states.
3. Add/update component tests around `ConversationList` or `Sidebar` to verify completed non-latest chain members receive the compact class and latest members do not.
4. Run the UI checks/tests relevant to `ConversationList`/sidebar styling, then `./dev.py check` if practical.

## Acceptance criteria

- Older completed chain conversations are materially more compact in the sidebar.
- The latest chain conversation still looks like the primary continuation target.
- Existing chain collapse/expand behavior, row navigation, menus, and keyboard navigation continue to work.
- No compact styling leaks to standalone conversations or non-sidebar list views.
