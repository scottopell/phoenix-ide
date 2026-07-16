# Make Coordinator mobile-first and restore its transcript

## Problem

The durable Coordinator shipped at `86f2ce14` with a desktop navigation entry and a desktop-first two-pane page, but without a complete mobile journey. On a phone there is no discoverable route from the conversation list, and direct navigation to `/global` renders a broken embedded conversation: state changes, the composer, and the state bar are visible, while transcript messages are not.

The immediate rendering failure is a layout-contract bug. Below 900px, `CoordinatorPage.css` changes `.coordinator-page` from a definite-height grid to an auto-height block and gives `.coordinator-conversation` only `min-height: 70vh`. Its embedded `#app` still uses `height: 100%`, but that percentage has no definite parent height. The conversation flex viewport and virtualized transcript therefore collapse while fixed-content siblings remain visible. The large blank card is the section's minimum height, not a functioning message viewport.

This violates the central product promise in `specs/global-recall/requirements.md`: the Coordinator must reuse Phoenix's standard transcript, composer, continuation, and persistence experience. It also shows that the completed task's mobile browser QA did not verify message visibility or the virtualized transcript's geometry.

## Product direction

Lead with the phone experience rather than stacking the desktop dashboard vertically.

On phone widths, Coordinator has two clear modes within one global surface:

1. **Conversation** — the default and dominant mode. The normal transcript fills the available viewport, remains independently scrollable, and keeps the composer/state bar usable with the keyboard and safe areas.
2. **Fleet** — a deliberate switch from Conversation. It presents the deterministic fleet projection as touch-friendly, attention-first rows with progressive disclosure.

A compact Coordinator header provides:

- a clear route back to Conversations,
- Coordinator identity,
- a visible Conversation/Fleet mode switch,
- fleet count/status without forcing the fleet list below the transcript.

This satisfies REQ-GR-010 by keeping the fleet snapshot available within the Coordinator surface without requiring both primary surfaces to compete for one phone viewport. Desktop retains the simultaneous conversation-plus-fleet layout; tablet behavior should be chosen from measured usable space rather than inheriting the phone or desktop structure accidentally.

## User journeys

### Enter and resume

- From the mobile conversation-list header, the user can open Coordinator through a labeled, touch-sized action.
- `/global`, a Coordinator notification, and `/global/:id` resolve to the same durable identity.
- The initial phone view is Conversation, showing existing transcript content—not merely runtime state.
- Browser back returns predictably to the prior app surface.

### Talk to Coordinator

- Existing and newly streamed user/assistant messages are visible.
- The transcript owns the remaining viewport between compact Coordinator chrome and conversation controls.
- The composer remains reachable when idle and while the software keyboard is open.
- Working, queued, error, recovery, context-full, and continuation states do not displace the transcript into a zero-height region.

### Inspect the fleet

- Switching to Fleet does not navigate away from the durable Coordinator route or remount/lose the conversation state.
- Rows prioritize project, title, presentation state, recency, and task/branch identity.
- The full row is a useful touch target; audit metadata is progressively disclosed.
- Copy-reference and open-conversation actions remain available without crowding the primary row.
- Returning to Conversation restores the prior transcript position and draft.

## Implementation plan

### 1. Restore the transcript as a P0 regression fix

- Give the embedded conversation an explicit, definite block-size contract at every responsive layout where it is visible.
- Make `/global` participate in app viewport ownership when Conversation mode owns the phone viewport, including correct document/touch containment.
- Preserve the standard `ConversationPage`/`MessageList`/`VirtualTranscript` path; do not introduce a parallel Coordinator transcript.
- Verify actual nonzero geometry for `#main-area`, `#messages`, and the virtualized rows, not just DOM presence.
- Audit the desktop pane at the same time so the fix does not trade one implicit height dependency for another.

### 2. Add a first-class mobile entry point

- Add a labeled Coordinator action to the mobile conversation-list header's utility navigation rather than hiding it in Settings.
- Keep New conversation and existing high-frequency actions dominant.
- Use the existing Coordinator route and accessible label consistently with desktop navigation.
- Mark the active global surface clearly when relevant.

### 3. Build the mobile Coordinator shell

- Add an explicit Conversation/Fleet mode model at phone widths, defaulting to Conversation on entry.
- Keep both modes under the canonical `/global/:coordinator-id` identity; use local UI state unless deep-linking a mode provides demonstrated user value.
- Ensure switching modes does not remount `ConversationPage`, reconnect SSE, clear a draft, or reset transcript position. Prefer CSS/layout visibility or a stable mounted slot with inaccessible hidden content handled correctly.
- Replace the oversized descriptive desktop header on phone with compact identity and navigation; retain richer framing where space permits.
- Respect iOS dynamic viewport units and top/bottom safe-area insets.

### 4. Redesign Fleet for touch and narrow widths

- Make the minimum row content scannable without horizontal overflow.
- Use large, unambiguous touch targets and avoid hover-dependent feedback.
- Move secondary operations such as Copy ref into expanded detail or an overflow treatment when needed.
- Preserve all transparency-contract data and stable app-local links; do not fabricate missing metadata.
- Preserve expansion within the Coordinator surface as required by REQ-GR-010.

### 5. Define tablet and desktop behavior explicitly

- Retain the productive side-by-side desktop composition when both panes have adequate usable width.
- Test the intermediate range rather than using one broad `<=900px` fallback. Tablet may use modes or a measured stacked treatment, but it must maintain a definite transcript viewport and avoid nested document scrolling.
- Keep Coordinator-specific CSS beside `CoordinatorPage` and avoid broad global overrides except for shared viewport-route ownership.

### 6. Add deterministic mobile QA coverage

Create a Coordinator Ladle/QA fixture following the repository fixture contract:

- deterministic scenarios for a populated idle transcript, active streaming/working state, long wrapped content, fleet expansion, and fleet load failure,
- realistic multi-message virtualized transcript data rather than a mocked placeholder,
- stable ready markers and no arbitrary sleeps,
- phone captures at approximately 390×844 and a second narrow/small-height viewport,
- tablet and desktop captures to guard responsive transitions,
- `./dev.py qa coordinator` integration and checked-in capture plumbing.

Targeted automated coverage must verify:

- mobile navigation exposes Coordinator,
- Conversation is the initial mobile mode and Fleet is reachable,
- mode switching preserves the mounted conversation and draft/transcript state,
- transcript scroller and visible message rows have nonzero geometry in browser QA,
- composer/state bar remain usable across idle and working states,
- no horizontal document overflow,
- fleet expansion and links are touch-accessible,
- canonical routing and notification deep links still work,
- desktop simultaneous layout remains intact.

Unit/jsdom tests should cover state and semantics; browser/fixture QA must cover geometry, scroll ownership, safe areas, and viewport behavior because jsdom cannot catch the shipped failure.

### 7. Correct documentation and completion evidence

- Update `specs/global-recall/executive.md` to describe the corrected responsive surface and honest verification coverage.
- Keep timeless requirements focused on user needs. Amend them only if the Conversation/Fleet availability contract needs explicit viewport-independent wording; do not add task or rollout history.
- Remove or supersede the stale claim that mobile browser QA was sufficient without transcript visibility evidence.
- Run the spec authoring pre-flight if a normative spec changes.

## Acceptance criteria

- A user can discover and open Coordinator from the mobile conversation list without typing `/global`.
- At supported phone widths, an existing multi-message Coordinator transcript is visible and scrollable on first render.
- Sending a message shows the user turn and streamed/final assistant content; runtime state is not the only visible evidence of activity.
- Conversation is the default phone mode; Fleet is one obvious tap away and its item count/status remains visible.
- Switching Conversation ↔ Fleet preserves the active SSE-backed conversation, draft, and transcript position.
- The composer, queued-working controls, state bar, transcript, and software keyboard coexist without zero-height or obscured primary content.
- Fleet rows are readable and operable by touch with no horizontal page overflow; expansion exposes the required audit metadata in place.
- `/global`, canonical Coordinator routes, notification routes, and back navigation behave consistently.
- Desktop keeps a usable simultaneous conversation/fleet layout, and intermediate viewport behavior is explicitly tested.
- Deterministic browser captures cover populated transcript, working/streaming, Fleet, expanded detail, and failure states at phone, tablet, and desktop sizes.
- Focused tests, TypeScript checks, Coordinator QA capture, and `./dev.py check` pass.

## Non-goals

- Changing the Coordinator's read-only authority or tool set.
- Adding background fleet analysis, steering, approvals, or multiple coordinators.
- Replacing the shared conversation runtime with Coordinator-specific message rendering.
- Redesigning every Phoenix mobile route; shared navigation changes should remain the minimum coherent shell needed for Coordinator discoverability.
