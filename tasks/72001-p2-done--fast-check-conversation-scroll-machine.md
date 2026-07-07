# Add fast-check coverage for conversation scroll state machine

## Goal

Add property-based tests for the new conversation scroll state machine, focusing on scroll safety, required liveness responses, valid reducer state shape, and browser/geometry compatibility. The tests should target the pure policy boundary rather than React/Virtuoso/DOM execution.

## Modeling decisions to make explicit

1. **Reducer boundary**
   - Treat the scroll machine as a pure transition from `(state, event, env)` to `(state, effects)`.
   - Keep DOM writes, Virtuoso calls, timers, and `Date.now()` outside the reducer.
   - Use logical time supplied by the test environment.

2. **Semantic event inputs**
   - Normalize production signals into policy facts before they reach the reducer:
     - `itemCount`
     - geometry: `scrollTop`, `scrollHeight`, `clientHeight`, `totalListHeight`
     - `growthOrigin`: `tail`, `nonTail`, or `none/shrink`
     - `now`
   - Avoid threading message type into the reducer. If production code needs message details, keep that in the adapter/classification layer.

3. **Pinned classification**
   - Treat “pinned” as a derived fact, not stored state.
   - Derive pre-growth pin distance from DOM metrics: previous DOM `scrollHeight`, current `scrollTop`, and the selected client height.
   - On viewport shrink, classify pinning using previous `clientHeight`.
   - Do not use virtualizer `totalListHeight` for pin distance.

4. **Settling state**
   - Represent settling explicitly, with a finite deadline.
   - Model deadline expiry and user engagement as transitions that stop settling without further corrective scroll.

5. **Effect taxonomy**
   - Make visible scroll effects explicit and mutually exclusive:
     - virtualizer `scrollToLastItem`
     - immediate DOM-bottom write
   - Make unread effects explicit and mutually exclusive:
     - show unread
     - clear unread

## Test plan

### 1. Global single-transition safety properties

Use fast-check to generate valid reducer states, events, geometry, and monotonic logical time. For every generated transition, assert:

- A transition never emits both virtualizer `scrollToLastItem` and immediate DOM-bottom write.
- A transition never emits both unread-show and unread-clear effects.
- Empty lists never emit `scrollToLastItem`.
- Jump-to-newest without content emits no visible scroll effect.
- Settling state always carries a finite deadline.

### 2. Auto-scroll safety properties

Add focused properties for content growth behavior:

- Engaged users who were not pinned before tail growth never receive a scroll-to-newest effect.
- Active upward gesture suppression overrides pinned proximity: pinned + suppressed + tail growth must not scroll.
- Ignored non-tail growth while not pinned emits neither visible scroll nor unread effects.
- Message type does not affect auto-scroll policy. Prefer making message type structurally unavailable to the reducer; otherwise add a metamorphic property where two inputs differing only by message type produce identical scroll-relevant state/effects.

### 3. Required liveness properties

Add focused properties with strong preconditions:

- Pinned, unsuppressed, engaged, non-empty users receive a scroll-to-newest effect from genuine tail growth.
- Suppressed genuine tail growth emits/sets unread state.
- First content after an empty measured mount enters settling and scrolls to newest.
- Jump-to-newest with content clears unread and scrolls to newest.

### 4. State-shape and reset sequence properties

Use generated event sequences to assert:

- Conversation identity changes reset scroll baselines, unread state, touch state, upward suppression, and engagement.
- Gesture suppression does not leak across conversation identity changes.
- Engagement is sticky until conversation identity reset or scroller reset.
- Upward wheel/scroll refreshes suppression.
- Non-upward movement does not refresh suppression.

### 5. Geometry/time compatibility properties

Add purpose-built generators for edge geometry:

- DOM-vs-virtualizer disagreement: when DOM metrics say pinned and virtualizer total height would say not pinned, reducer behaves as pinned.
- Viewport shrink: when `clientHeight` shrinks, pinned classification uses previous client height so a previously pinned user remains eligible to auto-follow.
- Settling after deadline exits/stops instead of correcting scroll.
- User engagement during settling stops settling instead of allowing further DOM-bottom correction.

## Implementation notes

- Keep arbitrary generators small and domain-constrained: non-negative heights, positive client heights, `scrollTop` within a reasonable scroll range, monotonic logical time.
- Prefer a compact semantic model over reproducing all production message-list details in the property tests.
- Add small adapter/unit tests separately if needed for mapping production signals into semantic inputs such as `growthOrigin: 'tail'`.
- Avoid flaky browser behavior: these tests should run in the normal UI test environment without real scrolling or timers.

## Verification

- Run the relevant UI test lane for the conversation scroll machine/property tests.
- Run `./dev.py check` before completion if the change touches shared UI code or generated/test infrastructure.

## Out of scope

- Browser-driven end-to-end scroll testing.
- Reworking Virtuoso integration beyond any small refactor required to expose a pure policy reducer.
- Exhaustively modeling every message type or render-unit shape inside fast-check.
