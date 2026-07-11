# Add reliable Cmd/Ctrl+F across Phoenix viewers

## Problem

Browser find reports matches that exist in the source data of virtualized viewers, but off-screen content is not mounted. The browser therefore cannot reliably highlight or navigate to those matches. This affects the Pierre-backed file and diff viewers and the React Virtuoso conversation transcript. Task approval and the other viewer bodies should expose the same predictable find interaction even though they are DOM-rendered rather than virtualized.

## User experience

Provide one compact, reusable in-app find bar for the active content surface:

- `Cmd+F` on macOS and `Ctrl+F` elsewhere opens find for the topmost eligible Phoenix content surface and suppresses native browser find.
- Opening find focuses and selects the query input. Typing searches the complete logical content, including currently unmounted virtualized content.
- The bar reports the current ordinal and total (`3 / 17`) or `0 / 0` without claiming matches it cannot navigate to.
- `Enter` / next and `Shift+Enter` / previous wrap through matches. Dedicated previous/next buttons provide the same actions.
- The active match is scrolled into view and visibly distinguished; other rendered matches are highlighted. Navigation remains correct when the target was initially unmounted.
- `Escape` closes find and returns focus to the previously focused element without closing the underlying viewer. A second `Cmd/Ctrl+F` while open focuses/selects the existing query.
- Search is case-insensitive literal text by default. Empty queries produce no matches. Query changes reset to the first match. Match ordering follows visual/document order.
- Find state is local to the mounted surface and is cleared when that surface or conversation changes. It must not leak between an underlying conversation and a topmost task/viewer overlay.
- Editable controls keep their expected text-editing behavior; the scope-routing tests must define and enforce when Phoenix intercepts versus allows browser/input behavior.
- Streaming or newly appended conversation content updates results without disrupting the current match when its stable identity still exists.

## Scope inventory

### Virtualized surfaces

1. **Pierre file viewer** — `MetaViewer` routes code and plain-text files to `PhoenixFileCodeView`. Search the full payload, map matches to 1-based lines and character ranges, navigate through `CodeViewHandle.scrollTo({ type: 'line', ... })`, and decorate rendered matches without depending on browser find.
2. **Pierre diff viewer** — `DiffView` / `PhoenixDiffCodeView`. Search filename/header text and displayed diff lines in actual visual order across committed then uncommitted items. Preserve side identity in split view and avoid double-counting the same context text merely because Pierre displays it in both panes. Navigate with typed item/line targets.
3. **Conversation transcript** — `MessageList` / React Virtuoso. Build a pure searchable projection from the same `RenderUnit[]` supplied to Virtuoso, retaining stable unit identity plus an intra-unit match locator. Navigate with `scrollToIndex`, wait for the row to mount, then reveal and highlight the exact rendered occurrence. Include user/agent/system/skill text and visible tool content represented by the unit; define explicit behavior for collapsed content and active streaming text rather than silently omitting it.

### Non-virtualized consistency surfaces

4. **Task approval** — `TaskApprovalReader` remains a separate phase overlay with local feedback semantics. Add the shared find bar and DOM-backed matching/navigation over rendered plan content; do not migrate it into the viewer slot or Pierre.
5. **Shared viewer bodies** — support Markdown, HTML source, large-text fallback, and conversation-message side viewer through `MetaViewer`/`ViewerShell`. HTML preview iframe content is excluded unless a safe same-document adapter can support it; make that limitation explicit in the UI/spec rather than showing false counts. Images and other non-text surfaces are ineligible.

During implementation, repeat the repository-wide inventory for any additional virtual scroller or text viewer introduced since this plan and either integrate it or document why it is not an eligible text surface.

## Architecture

### Shared find model and chrome

Create a viewer-find module with:

- Pure literal-match utilities returning stable typed match records, including source identity, source offsets, and an adapter-owned navigation target.
- A reducer/hook for query, ordered results, active result, wraparound navigation, result reconciliation as source data changes, and focus restoration.
- A compact accessible `FindBar` with labelled input, count/status announcement, previous/next, and close controls.
- A scope-aware shortcut hook. The topmost registered eligible surface owns `Cmd/Ctrl+F`; nested task approval/dialog/viewer scopes cannot trigger an obscured surface. Integrate with the existing focus-scope/keyboard model rather than adding independent global listeners that race.
- Surface adapters that supply searchable source data, navigate to a typed target, and apply/remove highlights. Keep Pierre and Virtuoso implementation details behind their existing wrappers.

Do not represent the same content as unrelated search and render strings. Search projections must be derived from canonical payload/render-unit data through pure, tested extractors, with stable identities that map directly back to the rendering target. If some rendered content cannot be reconstructed from typed data, add the missing typed projection rather than DOM-scraping unmounted content.

### Highlight strategy

Validate `@pierre/diffs` 1.2.0 capabilities before choosing decoration mechanics. Prefer a supported typed match/decoration API. If Pierre only exposes line navigation, extend the Phoenix wrapper with the narrowest reliable rendered-range decoration layer and test it against Pierre's real custom-element/shadow-DOM behavior. Do not couple search indexing or navigation to querying Pierre's virtualized DOM. Existing `unsafeCSS` line decoration may be used for line-level state, but exact substring highlighting is required where the renderer permits it; a line-only flash is not sufficient as the sole find indication.

For Virtuoso, index the full typed units, scroll by unit index, and use stable match metadata to decorate the mounted row after render. Avoid storing DOM nodes as match identity. Reapply highlights when markdown expansion, tool disclosure, or virtualization remounts the active unit.

## Implementation steps

1. Add timeless requirements for in-viewer find and update keyboard-interaction requirements for shortcut ownership, focus, Escape, and topmost-scope behavior. Add/update executive coverage maps. This is UI behavior and does not require Allium unless implementation reveals a genuinely cross-surface lifecycle that cannot be expressed clearly in requirements and tests.
2. Implement and unit-test the shared match model, reducer, keyboard routing, find bar, and focus restoration.
3. Add Pierre file and diff search projections and typed navigation targets. Extend `PhoenixFileCodeViewHandle` and `PhoenixDiffCodeViewHandle` with find navigation/decorations while preserving existing note and scroll-restoration behavior.
4. Add a conversation searchable projection beside the render-unit model, covering each discriminated unit/content type exhaustively. Integrate find with `MessageList` without destabilizing its existing scroll state machine, tail-follow behavior, conversation-nav jumps, or memoization boundaries.
5. Integrate the DOM-backed adapter into task approval and eligible `MetaViewer` bodies. Ensure annotation dialogs, notes panels, and confirmation dialogs retain topmost keyboard priority.
6. Add component CSS beside the owning shared find component; keep only genuinely global highlight primitives in `index.css` if required by Pierre's shadow boundary.
7. Update Ladle fixtures/scenarios for long file, multi-file diff, long conversation with off-screen matches, streaming append, task approval, and empty/no-match cases.

## Verification

Automated coverage must include:

- Literal matching, multiple matches on one line/unit, Unicode text, newline boundaries, case folding policy, ordering, and wraparound.
- Shortcut interception on macOS/non-macOS modifiers, repeated shortcut focus, editable targets, topmost scope, Escape, and cleanup after unmount.
- Pierre file navigation to an off-screen line and exact active/visible match decoration.
- Pierre unified and split diff ordering, side-aware targets, filenames, repeated/context lines, committed/uncommitted identity, and off-screen files.
- Virtuoso navigation to an initially unmounted unit, post-mount exact occurrence highlighting, repeated occurrences, collapsed content behavior, conversation changes, streaming/result updates, and no regression to tail pinning or conversation-nav pulse logic.
- Task approval and ordinary Markdown viewer navigation, with find Escape not dismissing the enclosing surface.
- Accessibility: labelled controls, keyboard-only operation, current/total status announcement, and visible focus.

Use real-browser QA in addition to jsdom/happy-dom tests because native shortcut suppression, selection/highlight APIs, Pierre shadow DOM, and virtualized remount timing cannot be validated faithfully by unit mocks alone. Exercise desktop macOS-style `Meta+F` and `Ctrl+F`, confirm native browser find does not appear on eligible surfaces, and verify real scrolling/highlighting in all fixtures. Run `./dev.py check` before committing.

## Acceptance criteria

- Browser find no longer reports unusable phantom matches on any eligible Phoenix virtualized viewer because Phoenix owns the shortcut there.
- Every reported match can be reached in both directions, including initially unmounted content.
- The active occurrence is visibly highlighted and centered/revealed; rendered non-active occurrences are highlighted distinctly where supported.
- File, diff, conversation, task approval, and shared text viewer behavior is consistent and keyboard accessible.
- Search cannot operate on or scroll an obscured lower scope.
- Existing annotation, note-jump, scroll restoration, conversation navigation, tail-follow, and viewer close behavior remain covered and passing.
- Specs and shortcut help/tooltips accurately describe the shipped behavior and any intentionally ineligible surface.
