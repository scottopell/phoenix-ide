# Inline message reactions that append to the current draft

## User need and agreed direction

While reading an LLM answer in the normal conversation transcript, the user wants to select a passage, immediately see a small reaction input, type a response, and append the selected quotation plus response to the existing message draft. Each reaction goes directly to the draft; there is no separate pending-note batch and no message is sent to the agent. Preserve the existing sidepanel/fullscreen reviewer and its batching workflow.

The user approved desktop and mobile mockups with one correction: the primary action must communicate appending, not copying to the clipboard. Use a text-lines-plus/list-plus icon and the explicit label **Add to draft** wherever space allows, with the same accessible name and tooltip for any compact icon-only rendering. Do not use a clipboard, paper plane, or send arrow for this action.

Desktop mockup: immediately visible compact input near the selected passage, close control, Add to draft action, and Cmd/Ctrl+Enter shortcut. Mobile mockup: native selection controls above/around the selected passage, reaction input below with space for selection handles. Merely selecting text must not steal focus or summon the keyboard. Clicking/tapping the already-visible input begins typing.

The mockups are exploratory interaction references, not production implementations. Their transient local files live in the originating Codex task's visualization directory; the behavioral requirements here must be sufficient to implement without those files.

## Evidence and owning boundary

- Existing `MessageContextMenu` resolves an owning message and handles `contextmenu` on the transcript container. It calls `preventDefault`, so native selection/menu coexistence must be considered at this shared boundary rather than by adding a second competing gesture handler.
- `MessageViewer` supports message-scoped line/block notes and batches the shared review-note collection via `formatNotesForSend`. This is the existing reviewer, not the desired immediate-append interaction.
- `ProductConversationPage` owns aggregate message identity and obtains the active/latest composer append capability from the latest embedded transcript projection. An older message's feedback must target the current conversation draft, not its historical transcript row.
- `ConversationPage` uses `useDraftActions` / `appendDraft` and explicitly requests composer focus for current viewer send flows. Inline reactions should reuse the draft authority without inheriting viewer close or composer-focus side effects.
- Browser-verified baseline: `product-conversation--desktop-multi-segment-qa-work` renders the real conversation page and composer; annotating older message #4, entering fullscreen with two notes, and sending feedback appends to an existing draft without submitting it.
- Existing metaViewer fixtures exercise isolated viewer states but have no-op send callbacks. The full conversation fixture has short messages and needs richer review content.
- The user reports no discoverable mobile entry to a conversation-message reviewer. The earlier browser pass verified desktop entry; touch-device/native-menu behavior remains unverified.

Authority starting points: `specs/conversation-ui/requirements.md`, `specs/prose-feedback/requirements.md` (including REQ-PF-017), `specs/viewer_slot/` normative artifacts, and `specs/keyboard-interaction/`. Read actual current authorities before implementation; update requirements and executive coverage, and record any changed interaction policy in the shared ADR chain.

## Scope and acceptance criteria

### 1. Select and react directly in the transcript

- [x] A non-empty selection wholly within a finalized assistant message opens the small reaction input immediately, without a second click on an activation icon. Support prose, lists, inline code, and code blocks within that message.
- [x] Capture the exact selected visible text and stable source-message/occurrence identity before focus moves to the reaction input. Do not replace it with an entire source line or truncate it to a short preview in the appended output. A display preview may be abbreviated.
- [x] Ignore selection in editable controls, unrelated surfaces, and selections spanning multiple messages. Do not offer the mutation action when the current conversation has no eligible draft/composer destination. Streaming/incomplete assistant content is outside this first implementation.
- [ ] Selection remains native: mouse drag, selection extension, keyboard selection, copying, right-click/system menus, mobile handles, and ordinary scrolling still work. Do not globally cancel pointer, selection, touch, or context-menu behavior to make the reaction UI work.
- [ ] The input appears without autofocus. It does not cover the selected text or selection handles; placement follows available room and stays inside the visible viewport. On mobile, account for browser chrome, safe areas, and the on-screen keyboard after the user taps the input.
- [x] The reaction input remains a single line with horizontal scrolling. Blank/whitespace-only reactions cannot be added. Cmd/Ctrl+Enter adds; plain Enter in the input does not append or submit. Unmodified Enter outside another control focuses the anchored input. IME composition must not trigger append.

### 2. Append atomically and keep reading

- [x] Add to draft appends a readable source reference, the selected quotation, and the user's reaction with sensible blank-line separation. Preserve the existing draft exactly, including edits made while the reaction input is open. Use the current draft store at activation time rather than a captured draft string.
- [x] Handle multiline selections and Markdown delimiters correctly so selected code/quotes cannot corrupt the surrounding feedback format. Preserve complete quote text and reaction text.
- [x] Each activation appends exactly once. No model request, queued message, clipboard write, or separate review-note entry results. Repeated sequential reactions append in the user's action order.
- [x] On success, clear the reaction input, dismiss the bubble, and provide a quiet accessible acknowledgement such as “Added to draft.” Preserve transcript scroll/reading position; do not autofocus or scroll to the composer. The normal composer remains editable and its Send action remains the only submission step.
- [x] Do not silently lose a typed reaction or reattach it to another source when selection changes, a transcript row virtualizes, or a viewer opens. Pin the source snapshot for a non-empty reaction; replacing it requires adding or explicitly discarding that reaction first. Empty pills can dismiss when the selection clears. Explicit close of a typed reaction offers Keep/Discard before clearing it.
- [x] On route/conversation changes, never append an old reaction into the new conversation. Integrate dirty-reaction handling with existing navigation/focus conventions. If the destination becomes unavailable while composing, retain the reaction and explain why Add to draft is unavailable instead of reporting success.
- [x] Inline reactions do not clear or submit notes already collected in the existing file/message/diff reviewer.

### 3. Mobile access and existing review workflows

- [ ] Native text selection reveals the inline reaction input without intercepting scrolling or long-press selection. Selecting alone leaves the keyboard closed; tapping the input opens it. Touch targets are at least 44 by 44 CSS pixels.
- [x] Add a discoverable per-message touch action/menu entry, **Open message reviewer**, that opens the existing message review interface without requiring right-click. Keep its existing annotation and batch-to-draft behavior. Do not offer a meaningless pane/fullscreen choice on narrow screens.
- [x] Desktop Open in sidepanel/Open in fullscreen remain available. Native selected-text actions and the application's message actions must coexist; an explicit message action affordance may provide app actions when native selection owns the context menu.
- [x] Validate focus and Escape ordering so closing an inline reaction does not also close an unrelated viewer or navigate the conversation. Do not trap keyboard users; expose meaningful input/action names and announce successful append without excessive selection announcements.

### 4. Realistic fixture and verification

- [x] Extend/add a deterministic full-conversation fixture using production transcript and composer components. Include a substantial assistant answer with several headings, long paragraphs, a list, inline code, a fenced code example, and a table, plus an older assistant message across a continuation boundary and a non-empty current draft. Avoid a separate imitation of the production interaction or no-op append callbacks.
- [x] Exercise two reactions to different passages, one older-message reaction, existing-draft preservation, editing the resulting draft, and no automatic submission. Verify source identity across historical segments, including repeated sequence numbers where applicable.
- [ ] Cover selection changes during typing, dismissal, rapid/double activation, unavailable destination, conversation navigation, virtualization/re-render, and existing-reviewer notes remaining intact with focused tests at the owning boundaries.
- [ ] Browser-verify the actual desktop selection-to-draft journey, multiline/code quoting, placement near viewport edges, no reading-position jump, and desktop native copy/context-menu behavior.
- [ ] Validate real touch behavior on iOS Safari and the installed PWA when available: native menu/handles, scroll without false activation, keyboard opening only on input focus, keyboard viewport placement, append, and the new reviewer entry. Also check Android Chrome. Desktop device emulation or WebKit automation alone is not evidence of native mobile selection coexistence; record unavailable device checks explicitly and do not claim they passed.
- [x] Capture desktop/mobile fixture evidence, update spec executive verification coverage, run focused tests and the required `./dev.py check`, and review the final diff.

## Suggested implementation sequence

1. Read the selection/context-menu, aggregate-message identity, draft-store, focus-scope, and virtualized-transcript boundaries; extend the realistic fixture first.
2. Implement a bounded selection/reaction lifecycle with a stable source snapshot and an explicit draft-append capability. Reuse the existing draft owner, not a second draft representation or the batch-note store.
3. Wire desktop and mobile presentation, native-interaction coexistence, keyboard behavior, and the mobile message-review entry.
4. Complete focused and real-browser/device acceptance, then update specifications/coverage and ship as one coherent feature.

## Limits and risks

This is a frontend selection-to-draft feature, not a new annotation persistence system. Do not add server APIs, database tables, cross-device pending reactions, refresh recovery for unfinished bubble text, permanent transcript highlights, threaded comments, or a redesign/removal of the existing reviewers. Once appended, text follows the existing draft's persistence contract.

The main uncertainty is native mobile selection/menu positioning and focus behavior; qualify that early. The other material risks are stale source/draft ownership across continuations or navigation, virtualized rows disappearing while typing, and shared menu/focus handlers suppressing ordinary OS interactions. Resolve these at their owning boundaries rather than through global event suppression.

## Implementation and verification — 2026-09-19

Implemented on `codex/inline-message-reactions` in the fresh worktree `/Users/scottopell/dev/phoenix-inline-reactions-45017`. Existing sidepanel/fullscreen and batch-note paths remain available. Typed unfinished reactions stay conversation-scoped in memory; reload recovery is outside scope.

- `./dev.py check`: all 16 checks passed. The UI suite includes seven focused reaction tests and a production-page fixture integration test exercising two sources with repeated sequence numbers, existing draft preservation, no submission, and opening the older reviewer.
- Chromium browser: desktop mouse selection immediately showed the unfocused input; Add to draft preserved the draft and reading scroll position. Native Cmd+C copied a selected multiline code excerpt. Adding that code reaction preserved its line breaks. The app context menu did not replace selected-text actions; this in-app browser did not expose an inspectable native context menu.
- At 390px width: selected an older answer, entered a reaction, and appended to the current draft with the historical occurrence identity. The Review touch action opened the existing historical-message reviewer. Screenshots of desktop and narrow layouts are retained in the originating Codex conversation.
- Reproducible fixture: `product-conversation--inline-message-reactions`, served locally at `http://127.0.0.1:61124/?mode=preview&story=product-conversation--inline-message-reactions` while the fixture server is running.
- Remaining acceptance: real iOS Safari, installed PWA, and Android Chrome selection menus/handles, keyboard viewport/safe-area placement, and touch scrolling. No physical-device session is available here. Keep this task in progress for that acceptance; do not treat desktop viewport checks as native mobile evidence.

## Revised interaction — fixture review before final integration

The user requests an always-small, single-line pill. Long input scrolls horizontally; no automatic growth or larger editor. The anchored row contains only reaction input, lines-plus Add to draft, and ×. Plain Enter does not submit; Cmd/Ctrl+Enter appends. The existing multi-line production presentation is not replaced until fixture feedback is incorporated.

An unfinished reaction whose selected passage leaves the transcript viewport becomes a compact dock above the composer. Its main action reads Return to passage with a reaction preview. Activating it uses production virtual-transcript navigation, waits for the source row to mount, restores the exact selected text range, and reopens the pill without focusing it. Manual scrolling back restores the pill too. Both anchored and docked states expose ×; typed reactions require Keep or Discard confirmation within the same compact row.

The Ladle inline-message-reactions story now includes 36 additional recovery-review exchanges across the earlier segment, using the production VirtualTranscript and composer. The pill presentation is injected only by this fixture. Ordinary routes retain the previous presentation pending user feedback and final integration. Offset-based passage restoration, navigation plumbing, and the existing shared reaction/draft stores exercise the real boundaries instead of a mock transcript.

Browser verification: a long typed reaction remained one line; scrolling eight screens removed the source message from the DOM and exposed the dock; Return to passage remounted it, restored the selected quote, and reopened the unchanged input without focus. Manual scrolling back also restored the pill. Docked dismissal offered Keep/Discard and Keep retained the text. Focused tests cover exact range reconstruction after remount, automatic undocking, long single-line input, IME/Enter behavior, and dismissal in both states.

Next: user tries this fixture and provides interaction feedback; incorporate that feedback before promoting the pill out of the fixture and updating the normative interaction contract. Native phone-device acceptance remains outstanding.

Validation for the fixture revision: `./dev.py check` passed all 16 checks. A browser-discovered empty-dismissal reopening bug was fixed with a focused regression test; the final `./dev.py check --lanes tsc,ui-lint,vitest` rerun passed all four UI checks, and fresh-browser empty dismissal stayed closed. The pill-to-production-draft append was browser-verified after the fixture revision.

## Approved integration and keyboard entry

The user approved the pill fixture and requested Enter-to-focus. The approved pill is now the production conversation presentation; fixture overrides and the superseded multiline bubble have been removed. Selecting alone still preserves native focus. Unmodified Enter focuses the anchored pill without scrolling, except when an input/control owns the key, a higher focus scope is active, or IME composition is in progress. Cmd/Ctrl+Enter remains the existing append action. The one-line input consumes plain Enter without submitting.

Browser verification used actual mouse selection followed by Enter, typing, and Cmd+Enter with no click in the input. The exact selected quote and reaction appended to the existing draft and no message was submitted. Regression tests cover normal input/button Enter behavior, modified Enter, IME, and the existing append shortcut. Native phone-device acceptance remains the outstanding task criterion; no deployment is included.

Integration validation: 19 focused tests passed. The broad `./dev.py check` passed every lane except a stale test expecting the superseded “Keep writing” label; that assertion was updated to “Keep”, and the full Vitest lane rerun passed. Browser verification confirmed Enter-to-focus and the unchanged Cmd+Enter append flow.

## Local adversarial review before PR

- Round 1: isolated virtualization and keyboard passes reviewed `d671971bba1b1a88bc5586e79360bf49277d4028..e1a414b8bceaf7a47ab1234a69a32dcf2eb204fd`. Two P2 defects reproduced: whole-message offsets drifted when earlier thinking/tool text changed; Enter intercepted native summary controls. Both fixed with regression tests.
- Passage anchors now use stable prose-fragment identity and local offsets, validate the recovered quote, and route exact-range positioning through VirtualTranscript. Mutable header/tool content is excluded from anchor ownership.
- Round 2: a fresh isolated full-feature review of `e2e98f9b720b0004e3fec8788ca3bcf61e0d0d88..4cf9069e35db7cfb865b082157b81e74cfc58437` found no further actionable defects, including an independent real-browser unmount/return journey.
- Round 3: an anchored regression challenge on that same revised range checked multiple prose fragments, duplicate message IDs with occurrence identity, changed preceding header/tool text, unmount/remount, and fail-closed quote validation. No actionable findings.
- Parent browser verification separately exercised ten-screen scrolling, actual source unmount, exact passage return, preserved reaction, and no autofocus. `./dev.py check` passed all 16 checks after rebase onto main. Physical-device mobile acceptance remains unverified.

These local results precede external Codex review; no comparison against Codex findings is claimed yet. User retains merge and deployment ownership.

## External review follow-up

Codex review on `6012c2de94a8d6582e33873af2307cad31ce6235` found a P2: a retained reaction could not return to a source outside the initially loaded history after leaving and reopening a conversation. The loaded-unit lookup rejected the source before requesting older pages. This violates the combined source-return and session-navigation promises in REQ-PF-018/020.

Comparison: **near-match / Codex-only / validated / isolated**. The local isolated round reviewed `4cf9069e35db7cfb865b082157b81e74cfc58437`; the external head is one documentation-only commit later with the same base and unchanged affected code. The missing review move was to discard the loaded-history cache while retaining the app-level reaction, then challenge return across multiple pagination boundaries. DOM unmount/remount alone did not exercise that boundary.

The follow-up routes source return through the existing history loader until the occurrence is available, with explicit completion, failure/exhaustion, and cancellation on navigation or discard. Regression coverage includes two-page lookup, immediate failure/retry without an intermediate loading render, pending-request cancellation, and a production-page cursor fetch restoring exact native selection without autofocus. The loader promise is preserved through both page owners and the navigation wrapper; the virtual transcript owns final positioning.

The second external pass on `009361ae1c386ad6f17122f19cb7b74a27c74a58` found two further valid P2s: Review was exposed for tool-only messages with no annotatable markdown, and primary-pointer media queries missed secondary touchscreens on hybrid devices. Review now shares the markdown-availability predicate used by the existing context menu; touch sizing uses `any-pointer: coarse`. A focused regression covers tool-only/blank omission and mixed prose/tool review with occurrence identity. Missing review moves: compare availability predicates across old/new entry points, and challenge primary versus secondary input-device capabilities. These are Codex-only findings; comparison with the prior isolated range is near-match because the intervening pagination change did not alter either affected affordance.


## Mobile native-menu overlap follow-up — 2026-09-20

Physical iPhone feedback showed the selection-relative pill overlapping the native Copy/Look Up/Translate menu. The bounded follow-up keeps the desktop floating `ReactionPill` unchanged and presents that same pill as an unfocused dock above `#input-area` for touch/coarse-pointer selection. The dock shows a shortened quote, captures the exact existing `ReactionSource` and native range on pointer-down before input focus, and follows existing `visualViewport` plus live composer geometry as the keyboard changes the visible viewport. It continues to use `InlineReactionStore`, draft append, Keep/Discard, and source-return; no note store, editor, persistence, menu-bound guessing, or global event suppression was added.

Focused coverage exercises existing-draft preservation and no send, no autofocus, source capture before focus clears selection, touch-versus-desktop presentation, and visual-viewport/composer movement. Chromium production-fixture verification confirmed a fine-pointer visible selection retained the floating pill, while a 390×844 touch-emulated path placed the dock 12px above the composer, preserved the existing draft, appended the exact source and reaction, dismissed the dock, and did not submit.

Physical iPhone Safari and installed-PWA acceptance for top/bottom selection, handle adjustment, scrolling, keyboard open/close, and simultaneous native-menu/dock usability was unavailable in this environment and is not claimed from emulation. The corresponding device criteria remain unchecked and the task remains in progress.
