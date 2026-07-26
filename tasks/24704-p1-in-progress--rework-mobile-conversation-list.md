# Rebuild the mobile conversation list around clear interaction requirements

## User problem

The mobile conversation list is laggy and buggy to scroll. Chains are especially poor: opening the final/current conversation expands every inactive intermediate link into a full-size row, consuming most of the viewport. The collapsed chain summary is already directionally good.

The implementation may freely replace the current list structure, state model, scrolling approach, and row design. Existing implementation details are not requirements. The current header is acceptable and is outside the redesign unless a structural change is required to make the list work.

## Product requirements

### Mobile scrolling

1. The conversation list SHALL scroll smoothly under touch with short and long datasets.
2. A normal upward or downward scroll gesture SHALL NOT trigger refresh, jump the viewport, lose momentum, or become trapped.
3. If pull-to-refresh remains in the product, it SHALL begin only from the actual top of the visible list, SHALL require an intentional downward pull, and SHALL fire no more than once per gesture.
4. The list SHALL have one unambiguous vertical scroll owner. Refresh gestures, overscroll behavior, list mutations, and any position handling SHALL all use that owner.
5. Periodic conversation updates SHALL NOT visibly move the list or rebuild unchanged rows while the user is scrolling.
6. Rendering cost SHALL remain bounded for realistically large conversation histories. The implementation SHALL use measured browser evidence to decide whether this requires windowing, progressive rendering, aggressive chain compaction, or a simpler architecture.

### Chain presentation

1. A collapsed chain SHALL remain a compact summary that identifies the chain and its latest/current conversation.
2. Opening the latest/current conversation SHALL NOT render inactive intermediate chain links as full conversation rows.
3. Inactive intermediate links SHALL use a dedicated dense history-link presentation. At minimum, each link must remain identifiable and selectable; extra metadata must be omitted unless it changes the user's decision.
4. The current member and any member requiring action SHALL remain immediately distinguishable from inactive history.
5. Expanding a chain SHALL preserve root-to-latest order without duplicating the latest/current summary.
6. Dense visual presentation SHALL retain accessible 44px touch targets even when the visible content is shorter or simpler.
7. Long titles, mixed-state chains, active non-latest members, archived chains, and long chains SHALL have deliberate, tested behavior.

### General list behavior

1. A row SHALL communicate, at a glance, its title, active/working/error/needs-action state, and whether it belongs to a chain.
2. Secondary metadata SHALL not be rendered merely because the desktop/full-row component already supports it. Mobile content earns space only when it helps choose a conversation.
3. Opening conversations and using rename/archive/delete/chain actions SHALL remain correct.
4. Keyboard navigation, focus, and screen-reader semantics SHALL remain correct where supported.
5. The existing header design, branding, command buttons, and active/archive controls SHALL remain visually unchanged unless a necessary scroll-container rewrite requires mechanical relocation.
6. The implementation MAY remove existing mobile-only behaviors, including saved raw scroll-position restoration or pull-to-refresh, when they conflict with these requirements and are not independently required by a normative specification.

## Verified implementation problems

- `#main-area` is the actual list scroller and scroll persistence reads `mainRef.current.scrollTop`, but pull-to-refresh checks `window.scrollY`. Since the window is not the list scroller, a mid-list downward gesture can satisfy the current top check and call `refresh()` during scrolling (`ConversationListPage`, `handleTouchStart` / `handleTouchMove`).
- The mobile root route is excluded from the shared viewport-ownership policy even though it uses a fixed-height app shell and an internal scroller (`DesktopLayout`, `viewportRoutes.ts`, `index.css`). Scroll ownership is split rather than explicit.
- `ConversationList` mounts every visible standalone row and every member of every expanded chain. Rows are memoized and unchanged list snapshots preserve references, but DOM/layout/paint work is not bounded.
- `isChainCollapsed` forces a mobile chain open when any member is active or actionable. `ChainBlock` then renders every member through the ordinary `ConversationRow` component.
- Chain density is coupled to terminal state: only inactive, non-latest, terminal members receive `conv-item-chain-completed`. Other inactive history uses the full row. There is no structural history-link representation.
- Existing tests cover default chain collapse, automatic active-chain expansion, completed-member styling, keyboard navigation, and generic touch containment. They do not cover pull-to-refresh against the real scroller, long-list performance, or an active final member with many compact predecessors.
- The mobile fixture covers semantic row variants but lacks a realistically long list and a long active chain suitable for performance and interaction QA.
- REQ-CONV-010 requires mobile single-column layout, safe-area handling, and 44px touch targets. REQ-CONV-012 requires at-a-glance state indicators. Existing normative specs do not require raw scroll-position restoration, pull-to-refresh, or the current chain-row design.

## Implementation plan

### 1. Establish the behavioral contract before redesigning

- Update the appropriate timeless conversation/chains requirements to capture the mobile scrolling and chain-presentation requirements above. Do not encode the current DOM/CSS structure or task/status references in normative specs.
- Decide explicitly whether pull-to-refresh and list-position restoration still earn their complexity. Remove either behavior if it cannot meet the requirements cleanly; do not preserve it by default merely because it exists today.
- Define the mobile visible-item model: standalone conversation, chain summary, inactive history link, current member, and actionable member. Make invalid presentation combinations difficult to represent.

### 2. Replace the list architecture as needed

- Give the mobile route one explicit scroll owner and route all relevant gesture/overscroll logic through it. Remove conflicting document/window checks and redundant containment code from this journey.
- Replace or substantially refactor `ConversationListPage`, `ConversationList`, chain rendering, and owning CSS as necessary. Reuse desktop components only when their semantics and render cost fit mobile.
- Separate chain-history density from backend conversation state. An inactive predecessor is a history link regardless of whether its stored display state happens to be terminal.
- Build one authoritative ordered visible-item sequence for rendering, navigation, focus, and active-item lookup rather than recomputing subtly different list shapes.
- Ensure store polling or unrelated atom updates preserve stable visible items and viewport position.

### 3. Measure first, then choose the performance mechanism

- Add a deterministic mobile fixture with many standalone conversations, several mixed-state chains, and at least one long chain whose final member is current.
- Before changing list rendering, capture throttled raw browser samples for long scrolling and chain expand/collapse. Record DOM count, scripting, rendering/layout, and interaction duration evidence.
- Use the evidence to choose the smallest architecture that satisfies the requirements. Windowing is allowed but not preselected; variable-height virtualization must not be introduced without proving it improves the measured journey and preserves focus/anchoring.
- Repeat the same scenarios after implementation and require a meaningful improvement beyond run-to-run noise.

### 4. Regression and journey coverage

Add tests and browser QA proving:

- a mid-list downward gesture scrolls normally and never refreshes;
- any retained pull-to-refresh fires once only from the true top;
- the long list remains responsive under throttled mobile conditions;
- opening the final/current chain member keeps all inactive predecessors in the dense history-link presentation;
- current and actionable members remain visually and semantically distinct;
- collapsed summary, expand/collapse, ordering, route selection, focus, menus, rename/archive/delete, and archived views still work;
- list updates do not cause visible viewport jumps;
- mounted item count is bounded if the chosen implementation relies on windowing/progressive rendering;
- the header is visually unchanged at representative phone widths.

Use the dedicated mobile fixture/Ladle route for screenshots and interaction QA, then run focused UI tests, typecheck/lint, and relevant `./dev.py check` lanes.

## Acceptance evidence

- At approximately 390px width, repeated fast and inertial scrolling through the long fixture is smooth, remains under the user's finger, and neither refreshes nor jumps unexpectedly.
- Opening the final/current entry of a long chain leaves inactive intermediate entries visibly compact; the current entry is obvious without scrolling through full-size predecessors.
- Collapsing the chain returns to the compact latest summary.
- Conversation updates arriving during a scroll do not visibly move the viewport or replace unchanged rows.
- Raw throttled before/after samples demonstrate a meaningful improvement and identify which changes produced it.
- Existing desktop behavior and all conversation/chain actions continue to pass.

## Explicit non-goals

- Redesigning the accepted header or unrelated mobile conversation transcript UI.
- Changing backend conversation APIs, chain membership/order, or persistence semantics.
- Preserving current list internals, CSS classes, pull-to-refresh, or raw scroll restoration for compatibility's sake.
