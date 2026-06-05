# Fix conversation breadcrumb jump alignment

## Problem

Clicking a conversation breadcrumb/chapter pill jumps to the target message, but the target is aligned as if the message list starts at the top of the viewport. Because the conversation breadcrumb/nav strip occupies the top horizontal slot, the first line of the target message can land underneath that bar and be hidden.

Likely affected code:

- `ui/src/components/ConversationNavStack.tsx` wires pill clicks to `MessageListHandle.scrollToUnitIndex`.
- `ui/src/components/MessageList.tsx` implements `scrollToUnitIndex` via Virtuoso `scrollToIndex`.
- `ui/src/components/BreadcrumbBar.tsx` still has a legacy DOM `scrollIntoView` path used by share/legacy breadcrumb rendering.
- `ui/src/index.css` defines `#conversation-nav` / `#breadcrumb-bar` height via `--breadcrumb-height`.

## Plan

1. Reproduce/confirm the overlap in a seeded conversation with enough messages to scroll.
2. Fix the jump alignment so the target message's top remains visible below the breadcrumb/conversation nav strip.
   - Prefer using the existing Virtuoso-backed path for conversation nav jumps.
   - If Virtuoso alignment is insufficient, add an explicit post-scroll offset correction against the `#messages` scroller and the visible nav bar height.
   - Keep the target highlight behavior intact.
3. Audit the legacy `BreadcrumbBar` click path and either make it offset-aware or route it through the same safer jump behavior where applicable.
4. Add/update UI tests for the jump behavior where practical:
   - unit test the jump handler/offset helper if extracted, or
   - component test that clicking a nav/breadcrumb pill invokes an offset-aware scroll path.
5. Run the relevant UI test suite and typecheck/check command before committing.

## Acceptance criteria

- Clicking a conversation breadcrumb/chapter pill scrolls the target message into view with its first line visible below the breadcrumb/nav strip.
- The target message still receives the brief breadcrumb highlight.
- Off-screen targets in the virtualized message list still work.
- Mobile/share/legacy breadcrumb behavior does not regress.
