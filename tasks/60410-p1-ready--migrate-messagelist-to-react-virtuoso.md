# Migrate MessageList virtualization from spacer-based progressive rendering to react-virtuoso

## Why now

After PR #161 (remove scroll-position memory) and the two follow-up
hotfixes (#162, #163) we still hit scroll-jump regressions in the hand-
rolled spacer-based virtualization. Browser-native `overflow-anchor`
would solve the bug class structurally but has no support in stable
Safari or any iOS Safari (Tech Preview only), and Phoenix's target user
runs latest macOS and iOS Safari + latest Chrome. The remaining options
are (a) ship a JS anchor-based comp ourselves, or (b) adopt
react-virtuoso, which solves the exact problem class — chat-style
bottom-pinned variable-height virtualized lists with cross-browser
scroll-anchor preservation — in a library that has been hardened over
years of edge-case work.

This task chooses (b). Rationale: rather than continuing to reinvent
windowing/anchor/follow-output primitives, adopt the SOTA library used
by Slack, Discord, Linear, and Notion for the same problem.

## Scope

### Replaces

- `ui/src/hooks/useBottomAnchoredWindow.ts` — the entire bottom-anchored
  window + IntersectionObserver expansion + spacer geometry +
  hand-rolled scroll-comp guard tower. Virtuoso owns windowing.
- `ui/src/hooks/useUnitHeightObserver.ts` — virtuoso measures heights
  internally.
- `ui/src/conversation/unitHeightCache.ts` — virtuoso owns its own
  measured-height cache.
- The MessageListBody spacer divs (`message-collapsed-spacer`), top/
  bottom sentinels, and the per-unit `data-render-unit-key` ref
  callbacks tied to the cache (the `data-render-unit-key` attribute
  itself stays for selectors used by tests / dev tools).
- The `useLayoutEffect`s in MessageList for ack-snapshot, scroll-comp
  baselines, and structural-change guards (all already deleted by
  PR #161; nothing more to remove there).
- The MessageList ResizeObserver-driven "pinned + new content → snap
  to bottom" logic — virtuoso's `followOutput="auto"` is the
  replacement.
- The `scrollTriggerRef = 'force'` force-scroll-on-system-message path
  (see "Behavioral change" below for the rationale).

### Stays unchanged

- The render-unit type model (`HistoricalUnit` / `TailUnit` discriminated
  unions in `ui/src/conversation/renderUnits.ts`). Virtuoso renders
  whatever JSX an item produces; we keep `buildRenderUnits`,
  `renderHistoricalUnit`, `renderTailUnit`, and the `pending_user as
  HistoricalUnit` invariant from REQ-MLRU-001 entirely intact.
- All message component rendering (`MessageComponents.tsx`,
  `StreamingMessage.tsx`, `MessageContextMenu.tsx`).
- The conversation atom / SSE wire / reducer plumbing (unchanged
  upstream).
- The streaming subscription isolation pattern (REQ-MLRU-010): leaf
  `<StreamingMessage>` subscribes to its own atom; virtuoso's
  `streaming_agent` tail item is just a wrapper.
- Jump-to-newest button affordance (we wire it to virtuoso's
  `scrollToIndex({ index: 'LAST' })`).
- `isPinnedToBottom` semantics — virtuoso exposes `atBottom` callback
  that fires when the user's bottom-pin state changes.

## Implementation outline

1. Add `react-virtuoso` to `ui/package.json` dependencies; run
   `pnpm install`. Verify lockfile clean.
2. Rewrite `MessageList.tsx` body to render a `<Virtuoso>` (or
   `<VirtuosoMessageList>` from `@virtuoso.dev/message-list` if its
   ergonomics match better — evaluate during implementation):
   - `data={[...historicalUnits, ...tailUnits]}` (single array, virtuoso
     doesn't distinguish — but tail units are pinned by being last and
     by virtuoso's `followOutput` behavior).
   - `itemContent={(_, unit) => renderUnit(unit, ...)}` — single render
     function that dispatches by `unit.kind`.
   - `computeItemKey={(_, unit) => unit.key}` — the existing render-unit
     key.
   - `followOutput="auto"` — pin to bottom when user is at bottom;
     don't yank when user has scrolled up. Replaces our pin-tracking
     logic.
   - `initialTopMostItemIndex={LAST}` — bottom-pinned mount per
     REQ-MLRU-005.
   - `atBottomStateChange={isAtBottom => setShowJumpToNewest(!isAtBottom)}`.
   - Use `firstItemIndex` to keep stable indices when prepending older
     units (virtuoso's recommended pattern for "load more above").
   - Sentinel-based expansion (REQ-MLRU-006) → virtuoso's
     `startReached` callback for top-of-list "load older content"
     (currently a no-op because Phoenix conversations are fully
     hydrated; future "infinite history" would use this).
3. Delete `useBottomAnchoredWindow.ts`, `useUnitHeightObserver.ts`,
   `unitHeightCache.ts`, and their test files.
4. Delete the spacer + sentinel DOM in `MessageListBody`. The body
   collapses to a single virtuoso instance.
5. Delete the `.message-collapsed-spacer` CSS rule.
6. Delete the `scrollTriggerRef` ref + the "hasSystem → 'force'"
   branch in the new-message detection block in `MessageList.tsx`.

## Behavioral change: relax REQ-MLRU-014 (drop force-scroll override)

The current code yanks the viewport to the bottom — and re-pins the
user — whenever a new `system`-type message arrives, regardless of
where the user was scrolled. The trigger comes from
`scrollTriggerRef.current = hasSystem ? 'force' : 'soft'` followed by
the ResizeObserver branch `if (isPinnedToBottom.current || trigger ===
'force')`. System messages in Phoenix include approval prompts, mode
transitions, and runtime errors — typically actionable, hence the
yank.

**This migration drops force-scroll.** Reasons:

1. Force-scroll is a hostile UX pattern. Slack / Discord / Linear /
   Notion never yank the viewport on incoming messages, however
   urgent; they use unread badges or inline indicators.
2. The current trigger is over-broad — it fires on any system message,
   including non-urgent transitions (cancellation, mode change).
3. Virtuoso's `followOutput="auto"` cleanly expresses "follow if
   pinned, leave alone otherwise." Force-scroll would require an
   imperative `<Virtuoso ref>` + `useEffect` watching for new system
   messages + manual `scrollToIndex({ index: 'LAST' })` — a special
   case bolted onto virtuoso's declarative model.

**Replacement (also in scope for this task):** the jump-to-newest
button already exists for unread content. When the unread set
contains a system message, the button should render with a visually
distinct urgency style (e.g. yellow background or `↓ Action required`
label) so users scrolled up can tell the new content is high-priority
without being forcibly moved.

**Spec impact:** REQ-MLRU-014 ("Pinned-to-Bottom Preservation") was
already simple — "pinned users stay pinned through new content, non-
pinned users see the jump-to-newest button." It will be reworded to
make explicit that no auto-scroll override exists for system messages.
The transparency-contract bullet "newest activity is visible without
having to scroll" is unchanged — it's satisfied for pinned users
naturally; non-pinned users get the visual cue.

## Spec changes

This is a large enough behavioral surface that the spec needs a
substantive rewrite. Plan:

- `specs/messagelist-render-units/requirements.md`:
  - Delete REQ-MLRU-005 (Bottom-Anchored Initial Window), REQ-MLRU-006
    (IntersectionObserver Boundary Expansion), REQ-MLRU-007 (Exact
    Scroll Compensation), REQ-MLRU-008 (Measured Spacer with Kind
    Fallback). Replace with a single requirement: "REQ-MLRU-015:
    Virtuoso-Owned Virtualization" naming virtuoso as the renderer and
    listing the configuration knobs that satisfy the transparency
    contract (followOutput, initialTopMostItemIndex, etc.).
  - REQ-MLRU-009 + REQ-MLRU-013 remain deprecated (already done).
  - REQ-MLRU-001 / 002 / 003 / 004 / 010 / 011 / 012 stay unchanged.
  - REQ-MLRU-014: reworded to drop the force-scroll-on-system-message
    override. New text: "pinned-to-bottom users stay pinned through
    new content arrival (virtuoso's `followOutput="auto"`); non-
    pinned users see the jump-to-newest button. The button SHALL
    render with a distinct urgency style when the unread set contains
    a `system`-type message. The SYSTEM SHALL NOT force-scroll the
    viewport for any message type."
- `specs/messagelist-render-units/windowing.allium`: delete entirely.
  Window lifecycle is now an internal concern of virtuoso, not Phoenix
  user code. The bottom-pin contract is restated in requirements.md.
- `specs/messagelist-render-units/render_units.allium`: unchanged. The
  render-unit construction model is upstream of virtualization.
- `specs/messagelist-render-units/design.md`: rewrite the "Window Hook"
  and "Height Cache" sections to document virtuoso configuration.
  Document the deleted hooks for traceability.
- `specs/messagelist-render-units/executive.md`: update status table —
  REQ-MLRU-005–008 → Deprecated; new REQ-MLRU-015 → Complete.

## Acceptance criteria

1. All 16 `./dev.py check` checks pass.
2. Switch between conversations (any two seeded fixtures) — every
   switch lands bottom-pinned, no random mid-list scroll positions.
3. Refresh a conversation — lands bottom-pinned.
4. Scroll up in a heavy conversation (e.g.
   `fixture-heavy-prod-shape`, 484 messages) — IntersectionObserver-
   equivalent expansion happens via virtuoso's startReached or
   internal windowing. No visible scroll jumps as older units come
   into view.
5. Send a message — pending bubble appears in place, transitions to
   acknowledged user message in place. No viewport jump. Same
   render-unit key persists through pending → sent (REQ-MLRU-001).
6. Streaming token arrival while at bottom — viewport follows the
   streaming buffer to keep it visible.
7. Streaming while scrolled up — viewport does not yank user down.
   Jump-to-newest button appears.
8. Clicking jump-to-newest snaps to bottom and clears the button.
9. System message arrives while user is scrolled up — viewport does
   NOT jump (force-scroll relaxation per REQ-MLRU-014 rewording);
   jump-to-newest button is rendered with urgency styling.
10. Performance: cold-load of `fixture-heavy-prod-shape` is no worse
    than the current spacer-based approach (measure via
    `browser_profile conversation-load` if available, else manual
    eyeball).
11. Net LOC reduction: the deleted hooks + spacer machinery should
    outweigh the virtuoso config code. Expect ~-300 to -500 LOC.

## Risks

- **Tool-result-heavy turns** (REQ-MLRU-012 regression test) need to
  render correctly under virtuoso's height measurement. Virtuoso
  measures items on first render and remembers; should be fine but
  worth a focused test.
- **Conversation switch identity** — virtuoso instances may not reset
  cleanly when `conversationId` changes. Mitigation: `key={slug}` on
  the `<Virtuoso>` element forces remount per conversation, at the
  cost of losing virtuoso's measurement cache across the switch. For
  Phoenix's session-length usage this is acceptable.
- **Mobile iOS Safari quirks** — virtuoso is well-tested but iOS
  momentum scrolling + bottom-pin can have edge cases. Verify
  empirically on a real device or simulator before declaring done.
- **License / bundle size** — virtuoso is MIT-licensed; gzip-compressed
  size ~12 KB. Acceptable.

## Out of scope

- Saved-scroll restoration. REQ-CONV-013 stays deprecated. Virtuoso's
  `getState()` / `restoreStateFrom` API exists but is not wired up.
- Cross-session height-cache persistence. Virtuoso's measurement
  cache is in-memory per virtuoso instance lifetime; same as our
  current REQ-MLRU-013-deprecated stance.

## Lineage

- PR #161: removed REQ-CONV-013 / REQ-MLRU-009 / REQ-MLRU-013, unified
  pending_user into HistoricalUnit.
- PR #162: spacer-baseline reset on conv switch (band-aid 1).
- PR #163: structural-change guard on spacer comp (band-aid 2).
- PR #164: tried CSS overflow-anchor; closed because Safari doesn't
  support it.
- This task: replace the entire spacer/window/scroll-comp layer with
  react-virtuoso.
