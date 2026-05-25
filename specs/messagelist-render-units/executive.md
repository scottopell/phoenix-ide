# MessageList Render Units — Executive Summary

## Scope and Boundary

This spec governs the **conversation message-list rendering pipeline**: how persisted messages, pending queued messages, sub-agent status, and the streaming view get folded into a single ordered list of render units, and how a bottom-anchored window virtualizes the historical portion of that list.

It is the structural follow-up to the bottom-anchored window introduced in task 65002 and the patch in `isRenderableHistoricalMessage` that excluded `tool` messages from the window count. Those landed for the immediate bug but left the model raw-message-oriented; this spec replaces the raw-message model with a typed render-unit layer.

**In scope:**

- Render-unit construction from `messages`, `pendingMessages`, and `convState` (pure transform)
- Pending user messages typed as `HistoricalUnit` sharing the eventual `user` unit's key, so pending → sent is an in-place keyed update
- Bottom-anchored window over `HistoricalUnit[]`
- Boundary-expansion sentinel and exact-scroll-compensation
- Measured per-unit heights (in-memory) with kind-estimate fallback
- Streaming view as a `TailUnit` kind with leaf-level buffer subscription
- Capability-gap logging on skipped messages

**Out of scope (removed):**

- Saved-scroll restoration — REQ-CONV-013 and REQ-MLRU-009 were deprecated; the entire save/restore/ack-snapshot path was removed.
- `sessionStorage` height-cache persistence — REQ-MLRU-013 was deprecated; the cache is in-memory only.

**Owned by other specs (cross-references, not duplication):**

- `specs/conversation_atom/` — owns the reducer that produces `messages`, `pendingMessages`, `convState`, and the streaming buffer this spec consumes
- `specs/conversation-ui/` — owns REQ-CONV-019 (streaming text display) at the requirements level; this spec is the structural implementation for the message list
- `specs/user_message_queue/` — owns the local queue that produces `pendingMessages`; this spec consumes them as `pending_user` `HistoricalUnit` instances

**Not in scope:** message bubble rendering itself (markdown, code blocks, tool blocks, copy buttons) — those live in `MessageComponents.tsx` and predate this spec. The render-unit layer dispatches to them by kind; their internals are unchanged.

## Requirements Summary

A correct-by-construction rewrite of the message-list virtualization so the virtualized window operates on the things that actually render, not on persisted database rows. The render unit becomes the structural truth: the rendered DOM list is `historicalUnits.slice(firstRenderedUnitIndex)` followed by `tailUnits`, with no filtering inside the render loop. Tool messages are owned by their `agent_turn` unit; sub-agent status and the streaming view are typed as `TailUnit` and cannot be accidentally collapsed by the window. Pending user messages are typed as `HistoricalUnit` of kind `pending_user`, keyed by `localId` — server ack populates `message_id = localId`, so the pending → sent transition is an in-place keyed update on a single render unit (no cross-region promotion, no ack-time scroll compensation needed).

## Technical Summary

- `ui/src/conversation/renderUnits.ts` — pure `buildRenderUnits({ messages, pendingMessages, convState, streamingHandle })` returns `{ historicalUnits: HistoricalUnit[]; tailUnits: TailUnit[] }`. Two discriminated unions; `agent_turn` carries `toolResultsByUseId: ReadonlyMap<string, Message>` and `isFirstInTurn: boolean` as fields, both computed at construction. Pending user messages append to `historicalUnits` keyed by `localId`. `streamingHandle` is a tag (`{ key } | null`) — the buffer itself is subscribed inside the leaf.
- `ui/src/hooks/useBottomAnchoredWindow.ts` — takes `HistoricalUnit[]` plus an optional `UnitHeightCache`. Returns `{ firstRenderedUnitIndex, spacerHeight, topSentinelRef }` where `topSentinelRef` is a callback ref so the IntersectionObserver wires up the instant the DOM node mounts (empty-then-grow conversations). Always bottom-pinned on mount. Exact-scroll-compensation preserved; an additional layout effect compensates spacer-height changes from measured-height writes.
- `ui/src/conversation/unitHeightCache.ts` — `Map<unitKey, number>` of measured heights, in-memory only (no sessionStorage mirror).
- `ui/src/hooks/useUnitHeightObserver.ts` — returns `observe` (memoized); `observe(unit)` is a stable per-unit-key ref callback that attaches a `ResizeObserver` and writes measured heights into the cache.
- `ui/src/components/MessageList.tsx` — derives units, slices, maps. Deletes `collapsedRenderableIds`, `isRenderableHistoricalMessage`, and render-time `inAgentRun` mutation. Subscribes to `useStreamingStartedAt(slug)` for the streaming-active signal (stable Object.is across tokens). The `<StreamingMessage slug={slug} />` leaf subscribes to the buffer directly via `useStreamingBuffer(slug)`.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-MLRU-001:** Render Unit Layer | Complete | `renderUnits.ts` with discriminated unions; pending user lives in `HistoricalUnit` with key = `localId` |
| **REQ-MLRU-002:** Tool-Result Structural Ownership | Complete | `toolResultsByUseId` built at construction |
| **REQ-MLRU-003:** Header Suppression by Construction | Complete | `isFirstInTurn` is a field, computed pre-window |
| **REQ-MLRU-004:** Tail-Pinned Unit Typing | Complete | `TailUnit` covers only `sub_agent_status` and `streaming_agent` |
| **REQ-MLRU-005:** Bottom-Anchored Initial Window | Complete | Window expressed in unit indexes; always bottom-pinned on mount |
| **REQ-MLRU-006:** IntersectionObserver Boundary Expansion | Complete | Sentinel callback ref; no scrollTop math |
| **REQ-MLRU-007:** Exact Scroll Compensation | Complete | Pattern preserved; spacer-height delta path added |
| **REQ-MLRU-008:** Measured Spacer with Kind Fallback | Complete | Per-unit `ResizeObserver` writes cache; spacer reads measured-when-present |
| **REQ-MLRU-009:** Unit-Anchor Saved-Scroll Restore | **Deprecated** | Removed alongside REQ-CONV-013; mount always lands pinned to bottom |
| **REQ-MLRU-010:** Streaming Subscription Isolation | Complete | `TailUnit` tag + `useStreamingBuffer` in leaf; commit-count regression test |
| **REQ-MLRU-011:** Capability-Gap Logging | Complete | `console.debug` on every skip path; no warn-or-higher |
| **REQ-MLRU-012:** Tool-Result-Heavy Tail Regression | Complete | Test exists in `MessageList.test.tsx` |
| **REQ-MLRU-013:** SessionStorage Height Cache | **Deprecated** | In-memory only; persistence served only the removed saved-scroll restore |
| **REQ-MLRU-014:** Pinned-to-Bottom Preservation | Complete | Existing `isPinnedToBottom` flow unchanged |

## Lineage

- Task 08300 (done): introduced react-window VariableSizeList over messages, dynamic height cache in sessionStorage. Superseded.
- Task 08322 / 08536 (done): feature parity + height-cutoff fixes on the react-window path.
- Task 65002 (done): rejected geometry-preserving render-virtualization; established the bottom-anchored window as the correct pattern.
- Task 65003 (ready): scripting/long-task tuning of the bottom-anchored window. This spec lands first; 65003 re-evaluates against the new model.
- Task 01004 (ready): the structural redesign this spec governs.
