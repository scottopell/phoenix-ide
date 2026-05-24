# MessageList Render Units — Executive Summary

## Scope and Boundary

This spec governs the **conversation message-list rendering pipeline**: how persisted messages, pending queued messages, sub-agent status, and the streaming view get folded into a single ordered list of render units, how a bottom-anchored window virtualizes the historical portion of that list, and how saved-scroll restoration anchors to units rather than pixel estimates.

It is the structural follow-up to the bottom-anchored window introduced in task 65002 and the patch in `isRenderableHistoricalMessage` that excluded `tool` messages from the window count. Those landed for the immediate bug but left the model raw-message-oriented; this spec replaces the raw-message model with a typed render-unit layer.

**In scope:**

- Render-unit construction from `messages`, `pendingMessages`, and `convState` (pure transform)
- Bottom-anchored window over `HistoricalUnit[]`
- Boundary-expansion sentinel and exact-scroll-compensation
- Measured per-unit heights with kind-estimate fallback, persisted to `sessionStorage`
- Saved-scroll restore by unit anchor `{ key, offsetWithinUnit }`
- Streaming view as a `TailUnit` kind with leaf-level buffer subscription
- Capability-gap logging on skipped messages

**Owned by other specs (cross-references, not duplication):**

- `specs/conversation_atom/` — owns the reducer that produces `messages`, `pendingMessages`, `convState`, and the streaming buffer this spec consumes
- `specs/conversation-ui/` — owns REQ-CONV-013 (per-conversation scroll position) and REQ-CONV-019 (streaming text display) at the requirements level; this spec is the structural implementation of both for the message list
- `specs/user_message_queue/` — owns the local queue that produces `pendingMessages`; this spec only consumes them as `TailUnit` instances

**Not in scope:** message bubble rendering itself (markdown, code blocks, tool blocks, copy buttons) — those live in `MessageComponents.tsx` and predate this spec. The render-unit layer dispatches to them by kind; their internals are unchanged.

## Requirements Summary

A correct-by-construction rewrite of the message-list virtualization so the virtualized window operates on the things that actually render, not on persisted database rows. The render unit becomes the structural truth: the rendered DOM list is `historicalUnits.slice(firstRenderedUnitIndex)` followed by `tailUnits`, with no filtering inside the render loop. Tool messages are owned by their `agent_turn` unit; pending queued messages, sub-agent status, and the streaming view are typed as `TailUnit` and cannot be accidentally collapsed by the window. Saved scroll restore uses a unit-anchor `{ key, offset }` rather than `savedScrollTop / estimatedRowHeight`, eliminating the "near-top saves disable virtualization" branch.

## Technical Summary

- `ui/src/lib/renderUnits.ts` — pure `buildRenderUnits(messages, pendingMessages, convState, isStreaming)` returns `{ historicalUnits: HistoricalUnit[]; tailUnits: TailUnit[] }`. Two discriminated unions; `agent_turn` carries `toolResultsByUseId: ReadonlyMap<string, Message>` and `isFirstInTurn: boolean` as fields, both computed at construction.
- `ui/src/hooks/useBottomAnchoredWindow.ts` — refactored to take `HistoricalUnit[]` plus an optional `SavedScrollAnchor`. Returns `{ firstRenderedUnitIndex, spacerHeight, topSentinelRef }`. Boundary expansion driven by `IntersectionObserver` on the sentinel; exact-scroll-compensation pattern preserved.
- `ui/src/lib/unitHeightCache.ts` — `Map<unitKey, number>` of measured heights, mirrored to `sessionStorage` keyed by `phoenix:hcache:{conversationId}:{unitKey}`. Cleared as part of the existing conversation-delete cascade.
- `ui/src/components/MessageList.tsx` — derives units, slices, maps. Deletes `collapsedRenderableIds`, `isRenderableHistoricalMessage`, and render-time `inAgentRun` mutation. Streaming buffer no longer passed as a prop; the `<StreamingMessage />` leaf subscribes to the streaming-buffer atom directly via `useAtomValue`.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-MLRU-001:** Render Unit Layer | Planned | Replaces raw-message map with typed unions |
| **REQ-MLRU-002:** Tool-Result Structural Ownership | Planned | `toolResultsByUseId` built at construction |
| **REQ-MLRU-003:** Header Suppression by Construction | Planned | `isFirstInTurn` is a field, not a render mutation |
| **REQ-MLRU-004:** Tail-Pinned Unit Typing | Planned | `TailUnit` separate from `HistoricalUnit` |
| **REQ-MLRU-005:** Bottom-Anchored Initial Window | Planned | Window expressed in unit indexes |
| **REQ-MLRU-006:** IntersectionObserver Boundary Expansion | Planned | Replaces `scrollTop - spacerHeight` heuristic |
| **REQ-MLRU-007:** Exact Scroll Compensation | Carried | Pattern preserved verbatim from prior pass |
| **REQ-MLRU-008:** Measured Spacer with Kind Fallback | Planned | Per-unit `ResizeObserver` writes cache |
| **REQ-MLRU-009:** Unit-Anchor Saved-Scroll Restore | Planned | Kills `savedScrollTop / 360px` estimation |
| **REQ-MLRU-010:** Streaming Subscription Isolation | Planned | `TailUnit` tag + leaf subscription |
| **REQ-MLRU-011:** Capability-Gap Logging | Planned | `console.debug` on every skip path |
| **REQ-MLRU-012:** Tool-Result-Heavy Tail Regression | Planned | Failing test prior to implementation |
| **REQ-MLRU-013:** SessionStorage Height Cache | Planned | First-paint spacer is exact across remounts |
| **REQ-MLRU-014:** Pinned-to-Bottom Preservation | Carried | Existing `isPinnedToBottom` behavior unchanged |

## Lineage

- Task 08300 (done): introduced react-window VariableSizeList over messages, dynamic height cache in sessionStorage. Superseded.
- Task 08322 / 08536 (done): feature parity + height-cutoff fixes on the react-window path.
- Task 65002 (done): rejected geometry-preserving render-virtualization; established the bottom-anchored window as the correct pattern.
- Task 65003 (ready): scripting/long-task tuning of the bottom-anchored window. This spec lands first; 65003 re-evaluates against the new model.
- Task 01004 (ready): the structural redesign this spec governs.
