# Refactor MessageList into a single anchor-aware timeline

## Problem

`MessageList` currently renders conversation content through two structurally different regions:

- `historicalUnits`, which participate in `useBottomAnchoredWindow`, spacer geometry, height caching, and saved scroll anchors
- `tailUnits`, which render after the historical window and are mostly outside that anchor/window model

That split makes pending user messages structurally different from their acknowledged server echoes. A pending message renders as a tail unit keyed by `localId`; when the SSE echo arrives, the same logical message is removed from the tail and appears as a historical user unit keyed by `message_id`. The recent scroll-preservation patch compensates for this promotion, but the bug class remains possible because the type/model still permits an ackable message to move across render regions.

Correct-by-construction goal: an ackable user message should be one timeline unit from the scroll model's perspective before and after acknowledgement. Pending → sent should be an in-place unit payload/delivery transition, not a cross-region remove/append.

## Desired design

Replace the historical/tail render boundary with a single ordered timeline model, for example:

```ts
type TimelineUnit =
  | {
      kind: 'user';
      key: string; // localId before ack; same value as server message_id after ack
      delivery: 'pending' | 'sent' | 'steering_queued';
      payload: QueuedMessage | Message;
    }
  | { kind: 'agent_turn'; key: string; ... }
  | { kind: 'system'; key: string; ... }
  | { kind: 'streaming_agent'; key: string; ... }
  | { kind: 'sub_agent_status'; key: string; ... };
```

The exact shape is flexible, but the structural rule is not: any unit that can later be acknowledged into `messages` must participate in the same anchor/window/height-cache model before and after acknowledgement.

`buildRenderUnits` or its replacement should derive delivery state from existing data, not introduce parallel message status state. “Sent” remains derived from server echo presence (`message_id === localId`).

## Implementation notes

- Audit `specs/messagelist-render-units/` before changing render-unit semantics; update specs/tests if the single timeline changes the normative model.
- Refactor render-unit construction so pending user messages and persisted user messages reconcile into one logical timeline position/key.
- Update `useBottomAnchoredWindow` to operate over the full rendered timeline, or create a successor hook whose input type makes out-of-band ackable tails unrepresentable.
- Distinguish truly ephemeral units (`streaming_agent`, `sub_agent_status`) from ackable units by type. If ephemeral units remain outside virtualization, their type must make it impossible to include `pending_user` there.
- Remove or simplify acknowledgement-specific scroll compensation once the cross-region promotion no longer exists.

## Acceptance criteria

- There is no separately rendered `pending_user` tail region for messages that can become historical messages.
- Pending and sent versions of the same user message share one canonical render-unit key and participate in the same anchoring/windowing model.
- The scroll/height-cache/windowing code consumes one ordered timeline, or a type split where only non-ackable ephemeral units can be outside that timeline.
- Pending → sent acknowledgement does not require bespoke cross-region scroll compensation.
- Existing bottom-pinned auto-scroll behavior is preserved.
- Existing non-bottom-pinned scroll preservation behavior is preserved.
- Regression tests cover acknowledgement while pinned and while not pinned.
- Render-unit specs/tests are updated to reflect the single-timeline invariant.
