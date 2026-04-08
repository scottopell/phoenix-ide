---
created: 2026-04-08
priority: p2
status: ready
artifact: ui/src/components/DesktopLayout.tsx
---

# Split pane layout: prose reader alongside chat

## Problem

Opening a file in the prose reader takes over the full main content area,
hiding the conversation entirely. To read code and follow the conversation
simultaneously, users must constantly toggle between views. On wide screens
(1440px+) there's enough horizontal space for both.

## What to build

A vertical split pane on desktop: chat on the left, prose reader on the
right. When a file is opened:
- If screen width >= threshold (e.g., 1280px), show side-by-side
- If below threshold, keep current full-screen overlay behavior
- Draggable divider to resize the split (or fixed 50/50 with a toggle)

### Layout

```
+----------+------------------+------------------+
| Sidebar  |  Chat/Messages   |  Prose Reader    |
|          |  (InputArea)     |  (File content)  |
+----------+------------------+------------------+
```

### Interaction

- Click a file reference in chat -> opens in prose reader pane (no overlay)
- Click a file in the file explorer -> same
- Close button on prose reader -> collapses back to chat-only
- Keyboard shortcut to toggle the split (e.g., Cmd+\)
- Prose reader pane remembers its width across sessions (localStorage)

### Mobile

No change -- prose reader stays as a full-screen overlay on narrow screens.

## Done when

- [ ] File opens in side-by-side pane on wide screens
- [ ] Chat remains visible and interactive while reading a file
- [ ] Prose reader pane can be closed to restore full chat width
- [ ] Narrow screens fall back to current overlay behavior
- [ ] Pane width persists across sessions
