# Narrow-WebKit Markdown table readability

Incident: GitHub issue comment 5668922043 reports poor readability for a narrow-WebKit Markdown table using the exact `Merge | What it unblocks` content with rows for #760 spec, follow-up work, and #769.

Historical context:

- PR #580 / task 81011 fixed inline-code font inheritance inside conversation Markdown tables.
- That prior root cause is historical evidence only; it is not proven to be the same cause here and must not be reused as the explanation without measurement.

Acceptance:

- Use the exact `Merge | What it unblocks` Markdown table fixture with #760 spec, follow-up, and #769 rows.
- Measure computed and rendered sizes for `th`, `td`, `strong`, and `code`.
- Measure `text-size-adjust`, `max-content` table breakout behavior, local table overflow, and document-level overflow.
- Distinguish intentional user horizontal scrolling inside the table from an inaccessible initial edge or content clipped outside the viewport.
- Verify actual Global Coordinator and ordinary/ProductConversation conversation surfaces, both finalized and streaming.
- Verify dark mode, light mode, desktop width, and narrow WebKit behavior.
- Preserve zoom and accessibility text scaling; do not globally disable browser text adjustment.

Non-goals:

- Do not relitigate PR #580 unless measurement proves a direct regression of the inline-code inheritance rule.
- Do not redesign Markdown parsing or table semantics.
