# Let wide conversation tables use the available chat viewport

## Problem

Conversation messages are intentionally capped at an approximately 800px centered column for readable prose. Markdown tables inherit that same cap, so wide tables require substantial horizontal scrolling even when the conversation pane has unused space on both sides.

The existing `.markdown-table-scroll` wrapper correctly localizes horizontal scrolling, but its `max-width: 100%` is resolved against `.virtuoso-row`, whose `max-width: 800px` prevents the table from using the wider chat viewport.

## Desired behavior

- Keep ordinary prose, message chrome, and normal-width tables in the readable centered column.
- Allow a table whose intrinsic content is wider than the prose column to expand symmetrically into unused space.
- Limit expansion to the current conversation scroller, with normal chat gutters; do not use the browser viewport when sidebars, file explorer, or another split pane narrows the chat.
- If table content is wider than the available conversation pane, retain horizontal scrolling on the table wrapper only.
- Preserve current behavior on narrow and mobile layouts.

## Risk boundary

`#messages` (the Virtuoso element) is the conversation's vertical scroll owner. `MessageList` keeps a ref to it and coordinates `scrollTop`, `scrollHeight`, pin-to-bottom state, range changes, total-list-height changes, and jump correction. Its slot component identity is also deliberately stable to prevent remounts and measurement hitches. This change must therefore **not** alter:

- the Virtuoso component tree, props, key, slots, item keys, or refs;
- the `#messages` width/height/display/position or vertical-overflow rules;
- `.virtuoso-row` width, because all conversation render-unit types share it;
- any scroll-machine or measurement code.

The available-width reference will instead be `#chat-view`, the stable, non-scrolling parent outside Virtuoso. It already owns the chat padding and tracks the actual conversation pane after sidebar, file-explorer, and split-pane resizing. This avoids `100vw`, which would incorrectly size content beneath adjacent panes.

## Implementation plan

### 1. Prove the layout in isolation before changing production CSS

Use browser devtools or a temporary stylesheet against a seeded conversation to validate a CSS-only breakout:

- establish inline-size query context on `#chat-view`, not on `#messages`;
- make `.markdown-table-scroll` retain a 100%-of-prose minimum, use intrinsic width when wider, and cap at the `#chat-view` content width expressed in container-query units;
- center the wider wrapper relative to the prose column;
- keep `overflow-x: auto` on `.markdown-table-scroll` so table content beyond the cap scrolls locally.

Before implementation, confirm all of the following in the rendered DOM:

1. `#messages.clientWidth`, `overflowY`, and `scrollTop` are unchanged when the breakout rule is toggled.
2. `#messages.scrollWidth === #messages.clientWidth`; the breakout must not create a Virtuoso-level horizontal scroll range.
3. A table wrapper's right and left bounds remain within `#chat-view`'s content bounds.
4. The row's vertical size is stable except for the expected appearance/disappearance of the table's own horizontal scrollbar.

If a centered intrinsic-width breakout cannot satisfy item 2 without changing Virtuoso overflow behavior, stop and reassess rather than adding `overflow-x` or containment rules to `#messages` as an incidental fix.

### 2. Apply only the validated CSS boundary

- Add inline-size query context to `#chat-view` while retaining its existing flex sizing and padding.
- Update `.markdown-table-scroll` with the validated intrinsic width, conversation-pane cap, centering, and local overflow rules.
- Preserve the semantic table element and existing table styles, spacing, striping, hover states, and touch scrolling.
- Do not widen `.virtuoso-row`, `.message`, or `.message-content`; prose and other message content must remain unchanged.
- Include a safe fallback retaining the current 100%-wide local scroller if container-query units are unavailable.

### 3. Add regression coverage at the right levels

- Keep the existing component tests proving finalized and streaming Markdown both render the local `.markdown-table-scroll` wrapper.
- Add a static/style regression assertion that the sizing context belongs to `#chat-view`, not `.message-virtuoso`/`#messages`, and that the table wrapper still owns `overflow-x`.
- Use real-browser QA for geometry and scrolling; jsdom cannot establish intrinsic table widths, container-query units, or Virtuoso measurements.

### 4. Real-browser scroll and resize matrix

Run the following against a long seeded conversation with the test table away from the newest message:

- **Normal table:** remains aligned to and no wider than the prose column.
- **Moderately wide table:** expands symmetrically into available chat space and has no local scrollbar when it fits.
- **Extremely wide table:** stops at chat gutters and scrolls only inside `.markdown-table-scroll`.
- **Pane resize:** open and resize sidebar/file explorer/viewer panes; table bounds track `#chat-view`, never the browser viewport.
- **Scrolled-up stability:** record the first visible render-unit key and its top offset, resize a pane across the table, and verify the same anchor remains visible without a jump beyond normal Virtuoso resize compensation.
- **Pinned stability:** at the conversation bottom, resize across the table and verify the newest message stays pinned.
- **Local vs vertical gesture:** horizontally scroll the table, then vertically wheel over it; verify table `scrollLeft` and conversation `scrollTop` are independently owned.
- **Streaming/finalized transition:** render a streaming table through finalization and verify no new horizontal page/scroller range and no unexpected vertical jump.
- **Narrow/mobile width:** behavior collapses to the existing prose-width local scroller without lateral page overflow.

Capture `#messages` `clientWidth`, `scrollWidth`, `scrollTop`, computed `overflowX/overflowY`, and the visible anchor before/after the high-risk resize cases. Any Virtuoso-level horizontal range, changed vertical-overflow ownership, lost pinning, or repeatable anchor jump blocks the change.

## Acceptance criteria

- Prose remains capped at its existing readable width.
- Normal-width tables retain their current width and alignment.
- Wide tables use available horizontal space up to the conversation pane’s gutters.
- Tables do not render beneath or size through sidebars or split panes.
- Tables wider than the conversation pane remain locally horizontally scrollable.
- The conversation/page does not gain a horizontal scrollbar.
- Finalized and streaming Markdown tables behave consistently.
