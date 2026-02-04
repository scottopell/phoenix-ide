---
created: 2026-02-04
priority: p2
status: pending
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

- [ ] No flash of "No conversations" on initial load
- [ ] Show loading state immediately
- [ ] Only show "No conversations" after data has loaded and array is actually empty
- [ ] Smooth transition from loading to content
- [ ] Test with slow network to ensure no flash

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
