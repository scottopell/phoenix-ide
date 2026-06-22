# Show sidebar PR badge only on latest chain conversation

## Problem

The conversation sidebar now renders cached PR badges per conversation. For conversations that are members of a continuation chain, this shows the same PR badge on every chain member. That conflicts with the intended compact/minimal treatment of non-latest chain conversations: only the latest chain conversation should surface the PR badge.

## Scope

Update the sidebar conversation rendering so:

- Standalone conversations continue to show `conv.cached_pr` when present.
- Chain conversations show `conv.cached_pr` only for the latest chain member.
- Non-latest chain members do not render `.sidebar-pr-badge`, whether expanded or compact/collapsed/minimal.
- Existing PR badge styling/link behavior remains unchanged for rows that still show the badge.

## Implementation notes

The likely UI seam is `ui/src/components/ConversationList.tsx` in `ConversationRow`, where `SidebarPrBadge` is currently rendered whenever `conv.cached_pr` exists. Gate that render with chain context, e.g. standalone OR `isChainLatest`.

Add/adjust regression tests in `ui/src/components/ConversationList.test.tsx`:

- Replace or revise the current expectation that conversations sharing a work scope render duplicate badges.
- Add a chain-mode test with cached PR data on both root and leaf, asserting only the latest chain member renders `.sidebar-pr-badge`.
- Keep coverage that standalone conversations with cached PR data still render a badge.

## Validation

Run the relevant UI test(s), then the project check if time permits:

```bash
pnpm --dir ui test ConversationList.test.tsx
./dev.py check
```
