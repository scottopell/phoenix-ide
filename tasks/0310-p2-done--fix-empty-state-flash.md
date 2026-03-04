---
created: 2026-02-04
priority: p2
status: done
---

# Fix "No Conversations" Flash on Initial Load

## Summary

The app briefly shows "No conversations found" before showing the loading spinner, creating a confusing flash of incorrect content.

## Context

When the app loads, there's a race condition where the conversations array is empty (but not yet loaded), causing the UI to briefly show the empty state before switching to the loading state. This creates a poor user experience.

## Current Behavior

1. App loads → conversations = []
2. Shows "No conversations found" (incorrect)
3. Loading starts → shows spinner
4. Data loads → shows conversations

## Expected Behavior

1. App loads → shows loading spinner immediately
2. Data loads → shows conversations (or empty state if truly empty)

## Acceptance Criteria

- [x] No flash of "No conversations" on initial load
- [x] Show loading state immediately
- [x] Only show "No conversations" after data has loaded and array is actually empty
- [x] Smooth transition from loading to content
- [x] Test with slow network to ensure no flash

## Implementation Notes

```typescript
// Current problematic logic:
if (conversations.length === 0) {
  return <EmptyState />; // Shows even when loading!
}

// Should be:
if (loading) {
  return <LoadingSpinner />;
}
if (conversations.length === 0) {
  return <EmptyState />;
}
```

## Additional Considerations

- Check both ConversationListPage and any other lists
- Consider adding a minimum loading time (e.g., 200ms) to prevent spinner flash on fast loads
- Apply same pattern to archived conversations
- Test with IndexedDB cache (should still show loading briefly)

## Implementation Notes (2026-02-04)

- Fixed by simplifying the loading check from `loading && !isReady` to just `loading`
- The issue was that when `isReady` became true, `loading` was still true briefly
- This caused the component to render the ConversationList with empty data
- Now it correctly shows spinner until data is actually loaded
- Tested with cache cleared to verify no flash occurs
