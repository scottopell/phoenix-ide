---
created: 2026-02-07
priority: p3
status: ready
---

# Conversation Page Scroll-to-Top Not Working

## Summary

On the conversation page, attempting to scroll to the top of messages doesn't work consistently. Both `window.scrollTo(0, 0)` and setting `scrollTop = 0` on the messages container failed to scroll to the first message.

## Context

During testing, when viewing `/c/cnn-lite-design-aesthetic-analysis`, attempts to scroll to the top of the conversation didn't work:

```javascript
window.scrollTo(0, 0); // No effect
const container = document.querySelector('.messages-container');
container.scrollTop = 0; // Still showed same messages
```

The page seemed stuck showing messages from the middle/end of the conversation.

## Possible Causes

1. Virtualized message list intercepting scroll
2. CSS `overflow` settings preventing scroll
3. Auto-scroll-to-bottom logic fighting with manual scroll
4. Multiple nested scroll containers

## Acceptance Criteria

- [ ] Users can scroll to top of conversation
- [ ] Scroll position is preserved when navigating away and back
- [ ] Auto-scroll to new messages doesn't fight with user scroll
- [ ] "Jump to top" button or keyboard shortcut works

## Relevant Files

- `ui/src/pages/ConversationPage.tsx`
- `ui/src/components/MessageList.tsx` (if virtualized)
- Related CSS files

## Notes

May be related to task 319 (virtualized messages) or 322 (virtualized message list missing features).
