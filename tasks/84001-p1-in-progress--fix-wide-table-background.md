# Fix wide Markdown table background discontinuities

## Problem

Wide assistant Markdown tables now break out of the centered 800px prose card and use the conversation pane, but transparent body rows reveal different ancestors across that width. Inside the centered assistant card they reveal `var(--bg-secondary)`; outside it they reveal the conversation background. This produces vertical color discontinuities at both card edges, especially in light theme.

The root cause is the combination of:

- `.message.agent { background: var(--bg-secondary); }` being limited to the centered message width;
- the table breakout extending beyond that message;
- table/body/odd rows retaining transparent backgrounds;
- only even rows explicitly painting `var(--bg-secondary)`.

Before breakout, transparency happened to look correct because the entire table sat over the assistant card. The table must now own the base body-row surface it visually depends on.

## Implementation plan

1. Update the conversation Markdown table styles in `ui/src/index.css` so the table paints the assistant body-row base color across its full rendered bounds. Preserve the existing header color, border treatment, hover state, breakout geometry, and local horizontal scrolling. Do not broaden the assistant card or paint unused breakout-wrapper space outside the table.
2. Extend the focused style regression in `ui/src/components/MessageList.test.tsx` to assert that the wide table owns an explicit base background rather than depending on the narrower message ancestor.
3. Strengthen the existing `wide-markdown-table` fixture so it clearly exercises header, odd/even body rows, prose before and after the table, and enough intrinsic width to cross both centered-card edges. Add light-theme coverage because that is where the reported contrast is most apparent; retain dark-theme coverage to catch the same boundary under both palettes.
4. Verify in a real browser at desktop width that:
   - every table row has continuous coloring from its left through right table edge;
   - the centered prose/card background does not create vertical seams through the table;
   - header and hover colors remain continuous;
   - table bounds and local overflow behavior remain unchanged;
   - narrow/mobile tables retain their existing local-scroller behavior.
5. Run the targeted UI tests and the repository check through `./dev.py`.

## Acceptance criteria

- Wide table body rows do not change color at the centered assistant-card boundaries in light or dark theme.
- The background is painted only within the table bounds; unused conversation-pane space is not filled as part of the fix.
- Header, borders, hover states, breakout sizing, and table-owned horizontal scrolling remain unchanged.
- Finalized and streaming Markdown tables continue to share the same corrected styling.
- Automated regression coverage protects the table-owned base surface and the visual fixture covers both themes.
