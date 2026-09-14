# AskUserQuestion design proposal

Revision 3 · 2026-09-14 · Proposed for user approval, not implemented.

Owner: task 10005. Baseline: b56a657c5 (fixtures), product ce66022db.
This proposal is the implementation brief pending approval. Existing normative
specifications remain authoritative until the approved changes are recorded there.

## Decision requested

Approve one coherent redesign of the question panel on desktop and mobile,
including all six recorded defects, the interaction rules below, regression
coverage, and AUQ runtime integration validation. Approval does not authorize
merge or production deployment. No new provider, persistence, or offline subsystem
is part of this proposal.

## Product goal

Let a user understand a question, compare choices, add qualifications, and send
exactly the intended answer without losing context. Optimize for short decisions
while accommodating long descriptions, code previews, four questions, and touch.

Success means zero silent answer omissions, visible keyboard focus, readable
choices at narrow widths, and an explicit distinction between a draft selection
and a response actually sent to the agent.

## Recommended layout

The panel remains attached to the conversation. It has three regions: compact
context header, one scrolling answer body, and persistent action footer. The
conversation retains its identity/status outside the panel. No resizable splitter
or detached window is introduced.

### Desktop and wide conversation panes

- Adapt to the panel's available width, not the browser window. At 840 CSS px or
  wider, questions with previews use an approximately 60/40 option/preview split,
  with a 24 px gap, minimum 420 px option column, and minimum 320 px preview column. Including 48 px outer padding, the split requires 812 px; an 840 px panel breakpoint leaves breathing room. The content is centered and
  capped at 1200 px; ordinary questions use a readable maximum 80ch answer column.
- Options receive at least 420 px at the split threshold; never retain the old
  320 px maximum. Descriptions wrap in full and are never ellipsized.
- The context header shows `Question 2 of 3 · Output` and compact question
  navigation buttons. A one-question set omits progress/navigation chrome.
- The full question begins the body. A compact header summary (header label and
  progress) stays visible while scrolling. Long question text scrolls with the
  body, avoiding a second scroller and an oversized sticky heading.
- The preview is sticky within the answer-body scroll region only when its full visible block fits the usable body height; otherwise it remains in flow. Apply the same eight-line inline disclosure as the narrow layout to long previews. It is labeled
  `Preview — <selected label>`. It never changes on pointer hover.
- Default panel max-height is 65% of available conversation height, expanding only
  as content requires; a deliberate `Expand questions` control can use the full
  available conversation region, with `Restore conversation` to reverse it. This
  is a local presentation toggle, not a saved user preference.

### Mobile, narrow desktop panes, and zoomed layouts

- Below 840 CSS px, use one column. Below 480 px, use 16 px outer padding; otherwise
  24 px. No horizontal page scrolling at 320 px or 400% desktop zoom equivalent.
- At panel widths below 480 CSS px, use the full available conversation content region for the question
  panel (including narrow desktop panes); retain the existing app back/navigation and conversation identity. It is
  an in-page surface, not a new modal layered over the conversation.
- Choices come first. One preview appears after the entire choice group, before
  notes. Its heading names the selected choice. This keeps choices stationary
  while the user compares them; moving a large preview into each selected row
  would shift the next tap target. A short inline `Preview below` link on the
  selected choice scrolls to that preview and transfers focus to its heading.
- Show up to eight preview lines initially, with `Show full preview` / `Show less`.
  Expansion stays inline. Normal preview text wraps; code may scroll horizontally
  inside the code block only. Expanded content uses the answer body's vertical
  scroller, not another vertically scrolling pane.
- The footer uses two rows where necessary: `Use chat instead` on the first, Back
  and the primary action on the second. Minimum effective touch targets: 44×44 CSS
  px. Editable text is at least 16 px on phones. Footer includes safe-area padding.
- Use the actual visible viewport while the software keyboard is open. Keep the
  active textarea and caret visible; allow the body to shrink and scroll. If the
  usable height is below 360 px (landscape/keyboard/zoom), compact the context
  header to a single line and use an in-flow footer so fixed chrome cannot consume
  the entire interaction area. Do not scroll the outer transcript behind it.

### Shared visual language

Keep Phoenix fonts, colors, spacing tokens, and existing action styling. Use 14 px
minimum description text at normal scale, 16 px labels, normal body contrast, and
an 8 px spacing rhythm. Selected state combines native checked control and a
subtle row treatment. Focus has a distinct visible outline; color alone never
communicates selection, error, or completion. No decorative cards around every
section, no animated auto-advance, no disappearing descriptions.

## Answer interaction contract

| State or action | Required behavior |
|---|---|
| New question set | Every question starts unanswered; no implicit first selection, including previews. |
| Recommended choice | A model-authored recommendation is text, never consent or automatic selection. |
| Single choice | Clicking the labeled choice or using native radio keys selects it. Selection does not advance, submit, or focus notes. |
| Multiple choices | Native checkboxes independently toggle; no preview UI for multi-select. Serialize predefined labels in displayed order, custom answer last. |
| Other | Always available with a multiline text field directly below the Other row. Choosing Other reveals its field. Pointer activation or explicit Edit focuses the field; arrow-key radio selection keeps focus on the radio, and Tab enters the field. |
| Editing Other | Clicking/focusing the field never toggles selection off. Entering non-whitespace text selects Other. In multi-select, other checked choices remain checked. |
| Deselecting Other | Explicitly unchecking it excludes its text but retains its draft for re-selection. If a nonempty draft exists, show `Custom answer not included` beside the collapsed field. Editing it reselects Other. |
| Switching away from Other | In single-select, retain its text locally; show `Saved custom draft — not included` with an Edit action. Edit selects Other and opens the field. |
| Empty Other | Selected Other requires non-whitespace text, even when other multi-select choices are checked. Explain beside the field; do not silently drop it. |
| Notes | `Add notes (optional)` exists for every question, including multi-select and Other. Expanding focuses the textarea. Notes are per question, not per option. |
| Notes disclosure | Collapse retains text; a nonempty value changes the label to `Edit notes · included`. Clearing is explicit editing. Notes alone do not answer a question. |
| Preview absent | Selected option without a preview shows `No preview for this option` in preview-capable questions; never reuse another choice's content. |
| No selection | Preview-capable questions show `Choose an option to view its preview`. This empty state is compact. |
| Other preview | Custom field stays in the answer column. Preview area says `Your custom answer will be sent`; it does not duplicate editable text. |
| Back / question navigation | Retain answers, notes, custom drafts, and preview disclosure per question during the mounted question set. Restore selected input focus on returning. |
| Next | Available only when the current question is answered; helper text explains the missing answer. It advances without sending. |
| Jump to a question | Navigation buttons may visit unanswered questions. Completion marks use a check plus accessible `answered` text. No implication that visited means answered. |
| Final question | Button is `Send answer` for one question, `Send answers` for multiple. Enabled only when all are answered; identify remaining unanswered question labels and provide navigation. |
| Successful send | Close on confirmed response success, restore focus to the conversation composer, show `Answers sent`, reconcile with authoritative state. |

Whitespace-only custom input is invalid; trim only the outer whitespace at
serialization, preserving newlines. Preserve user-authored notes and internal
whitespace. No invented character limit, summarization, or content truncation.
Preview annotation is derived from the selected option when sending, never from
what was last hovered, previously selected, or merely displayed.

## Keyboard and assistive technology

- On appearance, focus the first radio/checkbox without selecting it. On question
  changes focus the selected choice, or the first choice if unanswered. Scroll
  focused controls into view using nearest alignment, without animated movement.
- Tab/Shift+Tab move through actual controls: question navigation, choice group,
  Other text when expanded, preview disclosure, notes, and footer. Do not hijack
  Tab to change questions. Native radio grouping handles its own tab stop and
  arrow selection; checkboxes use Tab and Space. Prevent handled navigation from
  triggering conversation/sidebar handlers without suppressing native behavior.
- Space toggles focused controls. Enter activates buttons. Enter inside any
  textarea inserts a newline. Remove double-Enter submission and timed advance.
- Cmd/Ctrl+Enter explicitly sends the entire completed set, including from a
  textarea. If incomplete, navigate/focus the first missing answer and announce
  why it cannot be sent. Ignore this shortcut during IME composition.
- `n`, outside editable controls and when AUQ is the active scope, opens/refocuses
  notes without toggling it closed. Global help documents all shortcuts.
- Escape inside an editor returns focus to its disclosure/Other control without
  clearing text. Escape in a disclosed sub-context closes that context first;
  subsequent Escape opens the dismissal confirmation. Escape in the confirmation
  cancels it. Preserve the established global scope stack and shortcut priority.
- Choice groups use fieldset/legend or equivalent labeling; every radio/checkbox
  has a real label and associated description. Nested textareas are outside the
  choice label, so editing cannot bubble into selection toggles.
- Dynamic messages use a polite status region; errors use an alert plus a stable
  visible message. Do not announce each keystroke or move focus on pointer hover.
- Read-only historical rendering exposes question/answer content as static
  content, not active-looking radios or disabled editing controls. No edit/send
  shortcuts register for that instance.
- Respect reduced motion, high contrast, 200% text scaling, and screen readers.
  Target WCAG 2.2 AA contrast and focus visibility; actual compliance requires
  rendered measurement and assistive-technology validation during implementation.

## Sending, dismissal, and asynchronous state

| Condition | User-visible behavior and invariant |
|---|---|
| Sending | Freeze the submitted answer snapshot. Show `Sending…`; disable answer editing, navigation, dismiss, and send until resolution. One request in flight. |
| Definitive no-mutation rejection | Keep drafts, restore editing, show actionable inline error; no success toast or panel close. Only a typed rejection guaranteeing this mutation was not accepted qualifies, and only when no earlier attempt remains uncertain; generic 5xx does not. A retry rejection does not clear uncertainty about an earlier attempt. |
| Transport failure / unknown outcome | Keep the submitted snapshot frozen and say `Could not confirm sending. Checking status…`. Reconcile authoritative request identity; no automatic mutation retry and no editing of the uncertain snapshot. |
| Same request still awaiting after reconciliation | A read may precede the original mutation. Offer `Retry same answer` with explanation `Your original answer may still be processing`; retry only the identical frozen snapshot. Keep edit/navigation/dismiss locked until authoritative resolution, or proof that every outstanding attempt cannot mutate. A per-attempt retry rejection alone never unlocks editing. Leaving the page remains possible under the existing in-memory draft limitation. |
| Request already answered/dismissed elsewhere | Remove stale editing surface and announce `This question is no longer awaiting an answer`; do not claim this device sent it. |
| Reconnection with same request identity | Retain in-memory draft and reconcile waiting state without remount/reset. |
| New question set or conversation | Reset draft by the authoritative pending-request identity, not matching question text. Never carry an answer into another request. |
| Reload / leaving conversation | No new persistence guarantee. Drafts are in-memory; refresh or leaving may discard them. Show a concise note when notes/custom text first becomes nonempty. Durable draft recovery is explicitly excluded. |
| Use chat instead / dismiss | Confirm `Use chat instead?` with `No answer will be sent. The agent will wait for your message.` Buttons: Keep answering / Use chat instead. |
| Confirm dismissal succeeds | Close panel, focus composer, announce `Questions dismissed. Send a message to continue.` No inferred refusal or permission to resume. |
| Dismiss fails | Keep panel and drafts with inline error. Unknown outcome follows the same reconciliation rule as send. |

The persisted pending state already owns a tool_use_id. Reuse the pair
(conversation ID, tool_use_id) as request identity: expose tool_use_id in the
pending-question client state; require it in both respond and dismiss payloads;
carry it through runtime events; compare it at admission and again in the actual
state transition that consumes the pending request. A mismatched or consumed
request cannot mutate a subsequent question set, even with identical text.
Request consumption and resume must occur once through the existing state-machine
commit boundary. No new receipt store, automatic retry engine, or alternative ID.

This is confirmed implementation scope: update web types/SSE schemas and callers,
phoenix-client.py, native iOS API/action/session plumbing, and all fixture mocks.
The browser mobile layout is redesigned; native iOS receives protocol adaptation
and qualification only. Older clients missing identity receive an actionable
reload/update error with no mutation. Document this explicit protocol contract in
AUQ and compatibility requirements plus the shared ADR; do not add an optional
identity fallback. Every supported caller must ship this adaptation together.

Dismissal uncertainty also freezes the operation: offer Retry dismissal of the
same identity, never a competing send while that dismissal may still commit.
If reconciliation itself fails, preserve the frozen operation and show Check
status again. Do not falsely label the answer saved or sent. No guarantee that
an unmounted draft survives refresh is added by this proposal.

Every async completion is scoped to its originating (conversationId, tool_use_id).
Success, error, focus restoration, reset, and control re-enabling may affect only
that matching request. A newer question set may arrive before the old POST returns;
a late callback must leave the newer panel and its focus/draft intact. Add a test
where the next question set arrives before the previous response resolves.

The cost of the bounded uncertain-send policy is intentional: editing remains
locked if the server cannot resolve the original operation. Check status and
Retry same answer remain available; leaving remains possible but forfeits local
draft recovery. A richer revise/cancel-after-uncertainty flow would require a
separate authoritative operation-status contract and is outside this approval.

## Six-finding closure map

| Finding | Design resolution | Required proof |
|---|---|---|
| Custom answer lost | Separate choice activation and editor events; explicit inclusion state | Click, type, reposition caret, deselect/reselect, submit; exact payload assertions |
| Stale preview | Derive from selected option, including absence | A→no-preview→B→Other; displayed label/content and outgoing annotation agree |
| Focus offscreen | Focus real controls; nearest scrolling | Traverse every choice at wide/narrow/short heights; focused bounds visible |
| Missing notes | One per-question notes capability for every answer mode | Plain, preview, multi, Other, back/forward, collapsed notes all preserve payload |
| Unnamed inputs | Semantic groups and associated labels/descriptions | Accessibility tree plus VoiceOver/NVDA journey |
| Cramped layout | Container-based split, readable options, single body scroll | Long-content fixtures and full conversation shell at matrix below |

## Contract changes requiring approval

1. REQ-AUQ-002: replace focused/hovered-preview ownership with selected-option
   ownership; allow stacked preview presentation in narrow panes.
2. REQ-AUQ-001/003: explicitly require unanswered initial state, persistent
   per-question notes across choice changes, Other inclusion rules, and payload
   ordering. These refine semantics without changing answer-map wire shape.
3. REQ-AUQ-004/007 and compatibility: require request-bound mutations, all-client protocol adaptation, and frozen-snapshot recovery after uncertain outcomes. Missing/stale identity is a no-mutation error; no legacy optional fallback.
4. Keyboard interaction: replace AUQ's legacy Tab-to-next, timed Enter advance,
   and double-Enter send with native control navigation and explicit send. Scope
   isolation, focus restoration, and global shortcuts remain mandatory. Record
   the AUQ native-navigation exception/clarification for REQ-KB-008 so movement
   through native controls is not confused with triggering lower-scope shortcuts.
5. Record the approved rationale in a new shared ADR; update requirements and
   executive status together with implementation. Do not edit the legacy
   design.md as the new design authority or rewrite old ADRs.

## Implementation work packages and release gate

1. Contract + semantic answer state: approved spec changes, one draft per pending
   request/question, derived completion/payload/preview, Other and notes regressions.
2. Accessible controls + responsive layout: native labels/keyboard, focus recovery,
   header/body/footer, container breakpoints, selected previews and disclosures.
3. Async boundary validation: actual waiting→respond→resume and dismiss→prose
   journeys using mock model/runtime; failures, reconnect, stale request identity,
   double submission, read-only history. Request identity threading across all supported clients is mandatory.
4. Integrated QA: extend fixtures, capture before/after, run regression and
   repository checks, audit the exact final diff. Mark task done only after all
   six findings and the validation gate below are satisfied.

Validate at panel widths 320, 390, 600, 839, 840, 1024, and 1440 px; heights 320,
568, 844, and 900 as representative pairs; narrow pane in a wide window; desktop
200%/400% zoom; light/dark/high contrast; touch and keyboard. Include real iOS
Safari keyboard and desktop Chromium plus Safari. At short heights the footer
may scroll but must be reachable without hiding the active editor.

Acceptance criteria: no horizontal page overflow; all choices/descriptions fully
available; focus/caret visible; footer actions reachable; no unintended selection,
advance, send, or dismissal; exact outgoing answers/notes/preview; no duplicate
resume; no draft reset on same-request reconnect; no drafts transferred to a new
request; no agent resumption on dismissal until explicit prose. Run VoiceOver on
Safari and a desktop screen-reader pass where available. An unavailable required
platform is a stated qualification gap, not a passing audit.

## Cost and scope audit

Ordinary work: semantic form controls, layout, answer derivation, fixture matrix.
Potential cliff: durable drafts/offline sync would introduce storage versioning,
request ownership, cross-device conflict resolution, and retention policy. Excluded.
Potential cliff: retry/idempotency redesign. Reconcile current state and reuse
existing request identity; do not add an automatic retry engine.
Potential cliff: new modal/preview window or draggable splitter. Excluded; use
inline disclosure and one local expand toggle.

This is an approval-ready design target, not proof of an implemented experience.
Prototype and written audit establish reviewability; real viewport, keyboard,
runtime, and assistive-technology checks are implementation release gates.

## Audit record

Independent read-only review completed on revision 1 by design_audit. Four
approval blockers were identified and incorporated in revision 2:

| Audit finding | Resolution |
|---|---|
| Split minima exceed 800 px | 840 px container breakpoint; explicit 812 px minimum math |
| Arrow-to-Other steals focus | Separate selection from explicit editor entry |
| Request identity missing from client/mutations | Required existing tool_use_id threading across web, CLI, iOS, admission, transition, fixtures, specs |
| Read-after-unknown-send can race original mutation | Freeze original snapshot; identical retry only; unlock only on guaranteed no-mutation rejection |

Tall-preview stickiness and measurable narrow-layout expansion were also clarified.
Complexity audit excluded durable drafts, cross-device draft sync, automatic
mutation retries, draggable splitters, and new preview windows. Request identity
threading remains a necessary cross-client contract change, not cosmetic work.

Second independent review identified two additional async refinements: a retry rejection cannot clear an earlier uncertain attempt, and late callbacks must be scoped to the originating request. Revision 3 incorporates both. Final independent verification of revision 3 confirmed both closures and reported no remaining design-consistency blockers. Ready for user approval as a design proposal. This audit
is of design consistency and scope, not a claim that future implementation passes
runtime, accessibility, or device tests.

## Review preview

The conversation includes an interactive, illustrative desktop/narrow layout showing selection, mixed preview availability, Other, and notes. It is not the production component or a full wizard simulation; Next and dismissal describe the proposed transitions. Header/footer stickiness, software keyboard, runtime, and complete form semantics are specified above and require implementation validation.
