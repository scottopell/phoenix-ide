---
created: 2026-02-05
priority: p3
status: ready
---

# Add Loading Skeletons Instead of Spinners

## Summary

Replace generic spinner loading states with skeleton screens that preview the upcoming content structure.

## Context

The current loading states show a centered spinner with "Loading..." text. Skeleton screens provide a better perceived performance by:
- Showing the structure of content before it loads
- Reducing layout shift when content appears
- Making the app feel faster even if load times are the same

## Current Behavior

```tsx
// ConversationListPage.tsx
{loading ? (
  <div className="empty-state">
    <div className="spinner"></div>
    <p>Loading...</p>
  </div>
) : (
  // actual content
)}
```

## Acceptance Criteria

### Conversation List Page
- [ ] Skeleton cards showing conversation item structure
- [ ] Animated shimmer effect (subtle gradient animation)
- [ ] 3-5 skeleton items shown during load

### Conversation Page
- [ ] Skeleton message bubbles
- [ ] Different widths for user vs agent messages
- [ ] State bar skeleton (already visible, but populate with placeholders)

### Transitions
- [ ] Smooth fade from skeleton to real content
- [ ] No layout jump when real content loads

## Technical Notes

- Use CSS animations for shimmer (no JS needed)
- Skeleton components can be simple divs with background gradients
- Consider a `<Skeleton />` wrapper component for consistency

## Design

### Skeleton CSS
```css
.skeleton {
  background: linear-gradient(
    90deg,
    var(--bg-secondary) 25%,
    var(--bg-tertiary) 50%,
    var(--bg-secondary) 75%
  );
  background-size: 200% 100%;
  animation: shimmer 1.5s infinite;
}

@keyframes shimmer {
  0% { background-position: 200% 0; }
  100% { background-position: -200% 0; }
}
```

### Conversation List Skeleton
```
┌────────────────────────────────┐
│ ▓▓▓▓▓▓▓▓▓▓▓▓▓                  │
│ ░░░░░░░░░  ░░░░               │
└────────────────────────────────┘
┌────────────────────────────────┐
│ ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓             │
│ ░░░░░░  ░░░░░░░              │
└────────────────────────────────┘
```

## See Also

- `ui/src/pages/ConversationListPage.tsx`
- `ui/src/pages/ConversationPage.tsx`
- `ui/src/index.css` - existing spinner styles
- Task 310 (empty state flash fix) - related loading UX issue
