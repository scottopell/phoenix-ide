# Add horizontal scrolling for wide markdown tables in conversation messages

## Problem

In the main conversation view, markdown tables that exceed the available message width are clipped horizontally. Users cannot scroll sideways to read hidden columns. This affects a common LLM output format for comparison matrices, triage tables, plans, and summaries.

Root cause observed during triage:

- Finalized agent messages render markdown via `ReactMarkdown` in `ui/src/components/MessageComponents.tsx` inside `.agent-text-block`.
- Streaming agent messages render markdown via `ReactMarkdown` in `ui/src/components/StreamingMessage.tsx` inside `.agent-text-block`.
- Table CSS in `ui/src/index.css` styles `.agent-text-block table` with `width: 100%`, borders, padding, etc., but does not provide a horizontal scroll container.
- The main area clips horizontal overflow (`#main-area { overflow: hidden auto; }`), so overflowing table columns become inaccessible.

## Proposed fix

Use a wrapper component for markdown tables in conversation markdown, rather than making the entire conversation or message horizontally scrollable.

### Implementation plan

1. Add a table renderer for finalized agent markdown in `MessageComponents.tsx`:
   - Render markdown `table` as:
     - outer `<div className="markdown-table-scroll">`
     - inner semantic `<table {...props}>{children}</table>`
   - Keep table semantics intact while localizing horizontal scroll to the table.

2. Apply the same table wrapping to streaming markdown in `StreamingMessage.tsx` so wide tables are readable while a response is still streaming.

3. Add CSS in `ui/src/index.css`:
   - `.markdown-table-scroll { max-width: 100%; overflow-x: auto; -webkit-overflow-scrolling: touch; margin: 12px 0; }`
   - `.markdown-table-scroll table { min-width: 100%; width: max-content; margin: 0; }`
   - Adjust existing `.agent-text-block table` margin rules if needed so the wrapper owns vertical spacing and avoids double margins.

4. Verify existing table styling still applies:
   - borders
   - header background
   - cell padding
   - striped rows
   - hover state

5. Add or update tests if there is an existing suitable React/component test harness for message markdown rendering. At minimum, manually verify with a markdown table containing enough columns to overflow the 800px conversation column.

## Non-goals

- Do not make `#main-area` horizontally scrollable.
- Do not make the entire `.message-content` horizontally scrollable.
- Do not transform markdown tables into responsive cards.
- Do not change markdown parsing behavior.

## Acceptance criteria

- A wide markdown table in a finalized agent message can be horizontally scrolled within the table area.
- A wide markdown table in a streaming agent message can be horizontally scrolled within the table area.
- Normal-width tables continue to fill the available message width and retain current visual styling.
- Code blocks retain their existing independent horizontal scrolling behavior.
- The conversation view itself does not gain a global horizontal scrollbar because of a wide table.
