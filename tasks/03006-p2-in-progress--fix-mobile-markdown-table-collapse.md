# Fix mobile Markdown table column collapse

## Observed journey

In the iPhone 14 Pro Home Screen Phoenix PWA, an assistant message's two-column Markdown table displays the first header “Area” and labels such as “Environment creation” at roughly character width, with huge rows and the second column appearing to encroach. The reported viewport is approximately 393 CSS px wide. This is a reported screenshot, not a locally verified device observation.

## Evidence and uncertainties

- The shared conversation Markdown table renderer is used by finalized and streaming messages. `ui/src/index.css` applies `table-layout: fixed` to two-column conversation tables at widths <=600px, while semantic compact, atomic, and numeric columns receive `width: 1%`. The classifier in `conversationMarkdownTableSemantics.ts` classifies from body content; the reported labels appear consistent with a compact column. This interaction is a hypothesis, not a proven root cause.
- `ui/scripts/capture-message-list.mjs` and message-list Ladle fixtures exercise narrow and desktop tables, but their current assertions do not establish readable individual cell widths. Existing DOM tests cannot establish browser geometry. Preserve useful prior table behavior; do not blindly reinstate historical layouts.
- The exact offending assistant response starts “There are still a few meaningful ones—including automatic staging and committing” and is in Coordinator transcript `@transcript:78ab5664-2d42-44b1-9493-81178c8c5452`. The Explore transcript tool returned `invalid_target` and `no_predecessors` here. Obtain the exact source from the Coordinator through an authorized handoff before treating a fixture as faithful. Its reported headers are `Area | Mutations still present`; its six labels are Environment creation, Branch refresh, Ignore setup, Fork-proposal approval, Cleanup / recovery, and Native Close, with long descriptions containing bold and inline code.

## Scope and acceptance

1. Reproduce the reported table through actual production Markdown/message components, in WebKit at iPhone 14 Pro portrait geometry and a standalone-relevant layout; capture pre-fix screenshots and measured wrapper/table/cell geometry, wrapping and overlap. Cover finalized and relevant streaming paths. Check actual CSS/classification rather than assuming the hypothesis. Keep artifacts outside build outputs.
2. Implement the smallest evidence-supported general layout fix. Preserve semantic accessible tables and selectable text, normal prose sizing, desktop layout, and local horizontal scrolling where necessary. Avoid global nowrap, blanket font shrink, and fixture-specific rules. Validate long labels, inline code, URLs, and narrow multi-column tables: no document-level overflow, overlapping cells, or character-width labels.
3. Add a regression that fails against the prior behavior and passes after the change, using the exact message as fixture data. Extend existing message-list/streaming geometry harnesses as appropriate, with WebKit and desktop controls, and update stale CSS assertions only to reflect the established intended behavior. Run bounded UI-only format/typecheck/test/browser checks; no local Cargo, rustc, or native builds.
4. Obtain independent review, address findings, create a PR, and follow required CI through passing results. Report exact source, cause, before/after geometry and screenshots, checks, review, CI, and limitations. WebKit emulation is not physical iPhone PWA acceptance.
5. Coordinate integration and delivery with the Coordinator. The separate release owner `e148c954-ec86-4433-8e6a-08704b41a9d7` owns Close RC20; do not interrupt or extend that release without reconciliation. Do not mutate or deploy production, create a competing release writer, or modify unrelated Close/gix behavior. Coordinator owns the release handoff; user outcome is a fixed delivered PWA, not merely a PR.

## Boundaries

The producer is assistant Markdown; the shared table renderer adds column metadata and a scroll container; responsive CSS determines browser geometry. This task does not change persistence, wire contracts, or native iOS. Real-device PWA verification and exact deployed evidence must be explicitly reported rather than inferred from desktop browser emulation.
