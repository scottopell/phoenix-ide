---
created: 2025-02-09
priority: p2
status: ready
---

# Preserve scroll position when navigating back to conversation list

## Summary

When a user navigates into a specific chat and then goes back to the conversation list, the scroll position is not preserved. The list scrolls back to the top instead of staying where the user was.

## Context

Reported on mobile (iOS 14 Pro). This is a common UX issue where users lose their place in a long list after viewing a conversation. The expected behavior is that returning to the list should restore the scroll position to where it was before navigating away.

## Acceptance Criteria

- [ ] Scroll position is preserved when navigating from conversation list → chat → back to list
- [ ] Works on mobile Safari (iOS)
- [ ] Works on desktop browsers
- [ ] Position is restored smoothly without visible jump

## Notes

- May need to use `scrollRestoration` API or manual scroll position tracking
- React Router has some scroll restoration utilities that could help
- Could store scroll position in session storage keyed by route
- Consider using `useScrollRestoration` hook or similar pattern
