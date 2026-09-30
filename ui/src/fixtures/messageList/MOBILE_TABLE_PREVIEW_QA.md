# Mobile table preview QA

Use the existing authenticated `/preview/<absolute-path>/index.html` route. Do not start a fixture server.

## Matrix

Run every Case option in both **Streaming** and **Final** modes at viewport widths:

- 320px
- 390px
- 430px
- 768px

Run in Chromium and the available Playwright WebKit engine. For each combination:

1. Wait for `html[data-message-list-fixture-ready]`.
2. Confirm the selected provenance caption, selected case table, and expected headers are visible.
3. In Final, click **Status** and confirm the selected table intersects `.virtual-transcript.message-virtual-transcript`.
4. Scroll `.markdown-table-scroll` to its rightmost end and inspect the final column.
5. Confirm links, inline code, SHAs, bold text, and numeric alignment survive.
6. Confirm browser zoom/scaling remains available and text is not shrunk by fixture CSS.
7. Change Case, mode, and alternative; confirm navigation/readiness resets to the new table.
8. Assert no console errors, failed requests, production API calls, or external requests.

## Geometry acceptance

For every rendered table:

- Cells in a column have one consistent `data-column-kind`: `numeric`, `atomic`, `compact`, `label`, or `prose`.
- Numeric columns use intrinsic width (`width: 1%`, `white-space: nowrap`) and no minimum width.
- A numeric rightmost column must not exceed `max(header scrollWidth, body cell scrollWidth) + 32px` by more than 8px of rounding.
- Two-column Wrap tables fit the local table viewport at phone widths when the compact label plus prose minima fit; they do not inherit Overflow's max-content width.
- Prose columns have a computed minimum width greater than 0 and retain useful multiword wrapping.
- No prose body cell with eight or more words renders narrower than `min(10rem, 44vw)` in Wrap or `min(18rem, 68vw)` in Overflow.
- Atomic short IDs/model names remain on one line. Compact phrase labels wrap only at normal spaces within `min(5.5rem, 26vw)` to `min(11rem, 42vw)`. Long categorical labels may also wrap at word boundaries.
- Page-level horizontal overflow is zero; unavoidable overflow belongs only to `.markdown-table-scroll`.
- Controls wrap as whole items; no label splits inside a word.

## Case shape inventory

| Case | Expected kinds |
|---|---|
| 11:01 status | compact, prose |
| Source roles | compact, prose |
| Ranking | numeric, compact, prose |
| Links + SHAs | label, prose |
| Message matrix | compact, prose, prose |
| Latency | label, numeric, numeric |
| Failure policy | label, prose |
| Store size | compact, compact |
| 6-column models | atomic, numeric, numeric, numeric, numeric, numeric |
