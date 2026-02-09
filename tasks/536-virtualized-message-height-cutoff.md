---
created: 2025-05-30
priority: p1
status: ready
---

# VirtualizedMessageList height cutoff bug

## Summary

Messages in the virtualized list are being cut off/clipped, showing only partial content (e.g., tool results show truncated text like "¢ 1c" instead of full output).

## Context

The `VirtualizedMessageList` component uses `react-window`'s `VariableSizeList` for efficient rendering. The current implementation has a height measurement issue:

1. `RowRenderer` renders content inside a div with `style` from react-window
2. The react-window style includes a fixed `height` property (default 100px estimate)
3. Content renders clipped on first paint
4. `useEffect` measures actual height and calls `setItemHeight`
5. `resetAfterIndex` is called to update sizes
6. But content remains visually clipped

### Root Cause Analysis

The issue is that the row wrapper div has both:
- `style={style}` which includes `height: <fixed-value>` from react-window
- Content that may be taller than that fixed value

The content gets clipped by the parent's fixed height. Even though we measure and update, there appears to be a timing or rendering issue.

### Solution Approach

Two possible fixes:

**Option A: Separate positioning from sizing**
```tsx
// Extract only positioning from style, let content size naturally
const { height, ...positionStyle } = style as CSSProperties & { height: number };
return (
  <div style={positionStyle}>
    <div ref={rowRef}>
      {/* content */}
    </div>
  </div>
);
```

**Option B: Use overflow-visible**
```tsx
<div ref={rowRef} style={{ ...style, overflow: 'visible' }}>
```

Option A is cleaner and more correct for variable-size lists.

## Screenshot

See `/tmp/shelley-screenshots/upload_20e650227eff3b8c.png` - shows multiple bash tool results with content cut off to single line.

## Acceptance Criteria

- [ ] All message content renders fully without clipping
- [ ] Scrolling remains smooth with many messages
- [ ] Height updates correctly when content expands (e.g., collapsible sections)
- [ ] No visible "jump" or layout shift when heights update

## Notes

- File: `ui/src/components/VirtualizedMessageList.tsx`
- Line 57: `<div ref={rowRef} style={style}>`
- react-window docs: https://react-window.vercel.app/
