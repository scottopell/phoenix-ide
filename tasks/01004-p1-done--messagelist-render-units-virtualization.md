# MessageList render-units virtualization

Redesign the bottom-anchored MessageList virtualization around explicit render units instead of raw persisted messages. This is the correct-by-construction path for chat virtualization: window over the things that actually render, not over database rows that may be skipped or folded into another row.

## Problem

The current bottom-anchored window uses a raw message index/count as its window model. That is structurally leaky because not every persisted message is a standalone rendered row:

- `tool` messages are skipped as standalone rows and rendered inline through their owning `agent` message.
- empty/invalid `system` messages may not render.
- queued pending messages and sub-agent status are rendered outside the historical message map.
- future grouping behavior may add more non-1:1 mappings.

This caused two review findings:

1. Many recent tool-result records can consume the bottom window while the owning agent row — the actual visible UI — is collapsed behind the spacer. The current patch filters renderable IDs, but the deeper model is still raw-message-oriented.
2. Saved-scroll restore exposes the limits of estimated spacer geometry. Rendering from the top for every saved scroll is correct but disables virtualization on common revisits; estimating how many rows are above a saved pixel offset preserves perf but can restore into the wrong content when row heights vary. The current compromise only disables virtualization for near-top saved offsets. The render-units implementation should replace this estimate-bound compromise with a structural/measurement-based restore model.

## Goal

Introduce a `RenderUnit` layer:

```ts
type RenderUnit =
  | { kind: 'user'; key: string; message: Message }
  | { kind: 'skill'; key: string; message: Message }
  | { kind: 'agent_turn'; key: string; agent: Message; toolResults: Message[]; isFirstInTurn: boolean }
  | { kind: 'system'; key: string; message: Message }
  | { kind: 'pending_user'; key: string; message: QueuedMessage }
  | { kind: 'sub_agent_status'; key: string; state: AwaitingSubAgentsState };
```

Exact shape may differ, but the invariant must hold:

> The virtualized list contains only renderable units. Non-rendering raw records cannot occupy virtualized slots.

Then window over `RenderUnit[]`, not `Message[]`.

## Correct-by-construction invariants

- A raw `tool` message is never a virtualized row by itself.
- Tool results are structurally owned by the agent/tool-use render unit that displays them.
- Window boundaries are expressed in render-unit indexes, not raw message indexes.
- The spacer height is based on collapsed render units only.
- Consecutive-agent header suppression (`isFirstInTurn`) is computed before windowing so revealing older rows preserves the same grouping/header behavior as a non-virtualized list.
- Pending queued messages and sub-agent status are represented as render units or explicitly documented as non-virtualized tail units; there must be no ambiguity about whether they count toward the window.
- The latest visible work is always inside the initial bottom window, even when the tail contains many raw tool result records.
- Saved scroll restore is not decided solely by `savedScrollTop / estimatedRowHeight`; if restore uses an estimate, it must be conservative enough to avoid landing inside a synthetic spacer, while still preserving bottom-window virtualization for common bottom-pinned revisits.

## Boundary expansion design

Replace scrollTop/spacer-height threshold heuristics with a structural boundary sentinel:

```tsx
<CollapsedSpacer height={collapsedHeight} />
<div ref={topWindowSentinelRef} aria-hidden="true" />
{renderedUnits.map(renderUnit)}
```

Use `IntersectionObserver` rooted at the scroll container with an appropriate top `rootMargin` to expand older units before the sentinel/blank spacer reaches the viewport. Expansion should be based on the actual DOM boundary, not reconstructed scroll math.

Keep exact scroll compensation when prepending newly revealed units:

1. capture `scrollHeight` before shrinking `firstRenderedUnitIndex`
2. update the window
3. in layout effect, add the delta back to `scrollTop`

## No library migration in this task

Do not adopt `react-window`, `@tanstack/react-virtual`, `react-virtuoso`, or another virtualized-list dependency in this task. The immediate goal is to make the in-house model structurally correct. A library can still be reconsidered later, but render units are the prerequisite either way.

## Acceptance criteria

- `MessageList` builds a deterministic `RenderUnit[]` from raw `messages`, `pendingMessages`, and `convState`.
- The virtualization hook accepts/render-controls `renderUnitCount` (or equivalent), not raw message count.
- `MessageListBody` renders units, not raw message rows.
- Tool-result-heavy tails are covered by a regression test: an agent message with many following raw `tool` messages remains visible in the initial bottom window.
- Boundary expansion uses an IntersectionObserver sentinel at the spacer/rendered-window boundary; no `scrollTop - spacerHeight` trigger heuristic remains.
- Saved scroll restore to top (`scrollTop=0`) renders real content at the top, not an estimated spacer.
- Saved bottom-pinned revisit keeps virtualization active; a saved scroll key alone must not force rendering every historical row.
- Saved mid-conversation scroll restore either uses measured render-unit geometry or a documented conservative fallback; it must not restore into an estimated spacer that skips the content the user was reading.
- Switch into a large conversation still lands pinned to bottom.
- No visible scroll jump when expanding older units.
- Streaming message updates remain outside the historical render-unit list unless intentionally modeled; token updates must not re-render the historical unit list.
- Existing behavior for system prompt, pending queued messages, sub-agent status, tool result inline rendering, jump-to-newest, and context menu remains intact.
- Add focused tests for render-unit construction and the scroll/virtualization edge cases above.
- Validate with browser_profile `conversation-load` and record before/after metrics if the implementation changes performance materially.

## Relationship to task 65003

Task 65003 tunes the scripting/long-task regression introduced by the accepted bottom-anchored window. This task is a deeper correctness-first refactor of the virtualization model. If this lands first, re-evaluate 65003 against the new render-unit implementation; it may change or obsolete some of the tuning work.
