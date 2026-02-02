---
created: 2026-02-02
priority: p3
status: ready
---

# LocalStorage Quota User Feedback

## Summary

When localStorage is full, the offline handling code logs a console warning but provides no user feedback. Users may not realize their drafts or queued messages aren't being saved.

## Current Behavior

```typescript
// In useDraft.ts, useMessageQueue.ts
try {
  localStorage.setItem(key, value);
} catch (error) {
  console.warn('Error saving to localStorage:', error);  // User never sees this
}
```

## Proposed Solution

1. Add a toast/notification system to the UI
2. When localStorage write fails, show a non-blocking notification:
   - "Unable to save draft - storage full"
   - "Unable to queue message - storage full"
3. Optionally: add a "Clear old data" action to the notification

## Acceptance Criteria

- [ ] User sees notification when localStorage write fails
- [ ] Notification is non-blocking (doesn't prevent using the app)
- [ ] Notification suggests remediation (clear data, or just awareness)

## Notes

This is low priority because:
1. localStorage quota is typically 5-10MB - hard to fill with just drafts/queues
2. Most users will never hit this
3. The app still works, just without persistence
