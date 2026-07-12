# MessageList Render Units — Executive Summary

## Scope and Boundary

This spec governs the **conversation message-list rendering pipeline**: how persisted messages, pending queued messages, sub-agent status, and the streaming view get folded into a single ordered list of render units, and how that list is virtualized for display.

The render-unit construction layer (REQ-MLRU-001 through 004) is unchanged by physical virtualizer implementation changes. Phoenix VirtualTranscript is the sole physical layout authority underneath it for windowing, measurement, scroll-anchor compensation, programmatic positioning, prefix continuity, and tail preservation. The platform-neutral physical contract lives in `specs/virtual-transcript/`; this spec owns the conversation-specific render-unit and scroll-policy adaptation into that contract.

**In scope:**

- Render-unit construction from `messages`, `pendingMessages`, and `convState` (pure transform)
- Pending user messages typed as `HistoricalUnit` sharing the eventual `user` unit's key, so pending → sent is an in-place keyed update
- VirtualTranscript-owned virtualization (windowing, scroll-anchor compensation, item-height measurement, programmatic positioning)
- Streaming view as a `TailUnit` kind with leaf-level buffer subscription
- Capability-gap logging on skipped messages
- Durable tail-follow ownership, unread policy, gesture handling, and bounded mount rescue (REQ-MLRU-014)

**Out of scope (removed):**

- Saved-scroll restoration — REQ-CONV-013 and REQ-MLRU-009 were deprecated; the entire save/restore/ack-snapshot path was removed.
- `sessionStorage` height-cache persistence — REQ-MLRU-013 was deprecated; VirtualTranscript owns the in-memory measurement cache per mounted conversation.
- Hand-rolled windowing, spacer geometry, IntersectionObserver-driven boundary expansion, exact-scroll-compensation — REQ-MLRU-005 / 006 / 007 / 008 were deprecated; VirtualTranscript owns this layer.
- Force-scroll-on-system-message override — REQ-MLRU-014 forbids any auto-scroll override for non-pinned users.

**Owned by other specs (cross-references, not duplication):**

- `specs/virtual-transcript/` — owns the platform-neutral physical layout, measurement, positioning, and conformance contract that MessageList adapts
- `specs/conversation_atom/` — owns the reducer that produces `messages`, `pendingMessages`, `convState`, and the streaming buffer this spec consumes
- `specs/conversation-ui/` — owns REQ-CONV-019 (streaming text display) at the requirements level; this spec is the structural implementation for the message list
- `specs/user_message_queue/` — owns the local queue that produces `pendingMessages`; this spec consumes them as `pending_user` `HistoricalUnit` instances

**Not in scope:** message bubble rendering itself (markdown, code blocks, tool blocks, copy buttons) — those live in `MessageComponents.tsx` and predate this spec. The render-unit layer dispatches to them by kind; their internals are unchanged.

## Requirements Summary

A correct-by-construction rewrite of the message-list virtualization so the virtualized window operates on the things that actually render, not on persisted database rows. The render unit becomes the structural truth: the rendered DOM list is `[...historicalUnits, ...tailUnits]`, with no filtering inside the render loop. Tool messages are owned by their `agent_turn` unit; sub-agent status and the streaming view are typed as `TailUnit` and cannot be accidentally collapsed by the window. Pending user messages are typed as `HistoricalUnit` of kind `pending_user`, keyed by `localId` — server ack populates `message_id = localId`, so the pending → sent transition is an in-place keyed update on a single render unit (no cross-region promotion, no ack-time scroll compensation needed).

## Technical Summary

- `ui/src/conversation/renderUnits.ts` — pure `buildRenderUnits({ messages, pendingMessages, convState, streamingHandle })` returns `{ historicalUnits: HistoricalUnit[]; tailUnits: TailUnit[] }`. Two discriminated unions; `agent_turn` carries `toolResultsByUseId: ReadonlyMap<string, Message>` and `isFirstInTurn: boolean` as fields, both computed at construction. Pending user messages append to `historicalUnits` keyed by `localId`. `streamingHandle` is a tag (`{ key } | null`) — the buffer itself is subscribed inside the leaf.
- `ui/src/conversation/scrollMachine.ts` — pure reducer for `scroll_policy.allium`, using discriminated `unmeasured`, `mount-rescue`, and `live` sessions; durable `following`, `reading`, `navigating`, and `returning-to-tail` modes; explicit touch gesture variants; reducer-owned unread truth; and bounded mount recovery. Live ownership has no release timeout.
- `ui/src/components/VirtualTranscript.tsx` — Phoenix-owned web physical layout authority that satisfies `specs/virtual-transcript/`: it consumes ordered render units, owns measured extents and spacers, emits visible-range/pinned/total-extent observations, and exposes imperative tail/index positioning plus visible-anchor capture.
- `ui/src/components/MessageList.tsx` — derives units, concatenates `[...historicalUnits, ...tailUnits]`, hands them to a single `<VirtualTranscript>` instance configured with `initialTail={allUnits.length > 0}`, `estimatedExtent={120}`, `overscan={600}`, and `key={conversationId}` to force a fresh instance per conversation. System prompt renders via the `header` slot. The component adapts VirtualTranscript, DOM, gesture, and timer events into scroll-policy events and interprets policy effects as VirtualTranscript or DOM scroll operations. Jump-to-newest is an absolute overlay outside VirtualTranscript, driven by pinned/unread state, dispatching a policy event that calls `transcriptRef.current.scrollToTail()`. The `<StreamingMessage slug={slug} />` leaf subscribes to the buffer directly via `useStreamingBuffer(slug)`.

## Status Summary

| Requirement | Status | Notes |
|-------------|--------|-------|
| **REQ-MLRU-001:** Render Unit Layer | Complete | `renderUnits.ts` with discriminated unions; pending user lives in `HistoricalUnit` with key = `localId` |
| **REQ-MLRU-002:** Tool-Result Structural Ownership | Complete | `toolResultsByUseId` built at construction |
| **REQ-MLRU-003:** Header Suppression by Construction | Complete | `isFirstInTurn` is a field, computed pre-window |
| **REQ-MLRU-004:** Tail-Pinned Unit Typing | Complete | `TailUnit` covers only `sub_agent_status` and `streaming_agent` |
| **REQ-MLRU-005:** Bottom-Anchored Initial Window | **Deprecated** | Replaced by REQ-MLRU-015; VirtualTranscript owns bottom-pinned mount |
| **REQ-MLRU-006:** IntersectionObserver Boundary Expansion | **Deprecated** | Replaced by REQ-MLRU-015; VirtualTranscript owns boundary expansion via bounded overscan |
| **REQ-MLRU-007:** Exact Scroll Compensation | **Deprecated** | Replaced by REQ-MLRU-015; VirtualTranscript owns the scroll-anchor contract |
| **REQ-MLRU-008:** Measured Spacer with Kind Fallback | **Deprecated** | Replaced by REQ-MLRU-015; VirtualTranscript owns measurement |
| **REQ-MLRU-009:** Unit-Anchor Saved-Scroll Restore | **Deprecated** | Removed alongside REQ-CONV-013; mount always lands pinned to bottom |
| **REQ-MLRU-010:** Streaming Subscription Isolation | Complete | `TailUnit` tag + `useStreamingBuffer` in leaf; commit-count regression test |
| **REQ-MLRU-011:** Capability-Gap Logging | Complete | `console.debug` on every skip path; no warn-or-higher |
| **REQ-MLRU-012:** Tool-Result-Heavy Tail Regression | Complete | Test exists in `MessageList.test.tsx` |
| **REQ-MLRU-013:** SessionStorage Height Cache | **Deprecated** | In-memory only; persistence served only the removed saved-scroll restore |
| **REQ-MLRU-014:** Durable Tail-Follow Policy | Complete | Discriminated lifecycle/follow/gesture state; no live ownership timeout; centralized unread; bounded mount rescue |
| **REQ-MLRU-015:** VirtualTranscript-Owned Virtualization | Complete | Single `<VirtualTranscript>` instance owns windowing, scroll anchoring, measurement, and positioning |
