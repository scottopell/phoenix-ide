---
created: 2026-04-04
priority: p2
status: in-progress
artifact: pending
---

# fix-iac-subtitle-layout-overflow

## Plan

# Fix: Slash skill tooltip subtitle layout overflow

## Root Cause (confirmed by inspection)

In `.iac-item` (horizontal flex row), `.iac-item-subtitle` has `flex-shrink: 0` with no `overflow: hidden`. The subtitle text ("React and Next.js performance optimization guidelines...") renders at its natural width — **2179px** — inside a **369px** dropdown. This:

1. Crushes `.iac-item-label` to `width: 0` (completely invisible)
2. Overflows the container hard at the right edge

Font sizes (13px / 11px) are correct and not the issue.

## Fix — `ui/src/index.css`

The item layout should stack label on top of subtitle (column), both truncating with ellipsis. This is the right UX for a skill name + long description, and works at any viewport width.

### `.iac-item` — switch to column layout
```css
.iac-item {
  display: flex;
  flex-direction: column;   /* stack label above subtitle */
  align-items: flex-start;
  gap: 1px;                 /* tight vertical gap */
  width: 100%;
  padding: 6px 10px;
  background: none;
  border: none;
  color: var(--text-primary);
  font-size: 13px;
  font-family: 'SF Mono', 'Fira Code', monospace;
  cursor: pointer;
  text-align: left;
  transition: background 60ms;
}
```

### `.iac-item-label` — full width, truncate
```css
.iac-item-label {
  width: 100%;
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
```
(Remove `flex: 1` — not meaningful in column layout.)

### `.iac-item-subtitle` — full width, truncate, allow shrink
```css
.iac-item-subtitle {
  font-size: 11px;
  color: var(--text-muted);
  width: 100%;
  min-width: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
```
(Remove `flex-shrink: 0`; add `width: 100%`, `overflow: hidden`, `text-overflow: ellipsis`.)

## Acceptance Criteria
- [ ] Skill description no longer overflows the dropdown container
- [ ] Skill name (label) is visible on the first line
- [ ] Description (subtitle) is visible below it, truncated with `…` when too long
- [ ] Items without a subtitle still render correctly (single label line, no gap)
- [ ] Looks correct on both 393px (mobile) and 1280px+ (desktop) viewports


## Progress

