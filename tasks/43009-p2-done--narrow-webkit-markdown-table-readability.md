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

Resolution evidence:

- Chromium did not reproduce font inflation: prose rendered at 16px/24px while `th`, `td`, `strong`, and `code` rendered at 14px/21px; the #580 inline-code inheritance rule remained intact.
- The distinct reproduced cause was intrinsic `width: max-content` growth: before the fix, the 390px viewport provided a 254px local wrapper around a roughly 1013px two-column table. The initial edge remained accessible at `scrollLeft = 0`, local horizontal scrolling owned overflow, and the document itself did not overflow.
- The shared finalized/streaming renderer records its structural column count on each conversation table wrapper. The narrow fixed-layout and wrapping policy targets exactly two-column tables; wider tables retain their local horizontal-scroll fallback. The fix does not set `text-size-adjust`, so browser zoom and accessibility autosizing remain available.
- Post-fix Playwright Chromium and WebKit 26.5 captures passed all ten message-list stories. At 375px in dark and light themes, the two-column table and wrapper rendered at 311px with `scrollLeft = 0`, an accessible initial edge, no document overflow, fixed layout, anywhere cell wrapping, and 14px/21px `th`, `td`, `strong`, and `code` text.
- At 960px, dark/light two-column tables retained auto layout at 784px without document overflow. Both wide-table fixtures retained local horizontal overflow at mobile width. Playwright WebKit reports its untouched default `text-size-adjust` computed value as an empty string rather than `auto`; the stylesheet contains no override.
- Ordinary and ProductConversation routes converge on `ConversationNavStack` and `MessageList`; coordinator fixtures use the same `MessageList`. Finalized and streaming renderers share the conversation table wrapper contract.
