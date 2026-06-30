# MessageList Render Units — Design

This document describes the technical architecture implementing the
requirements in `specs/messagelist-render-units/requirements.md`. The
behavioural specification for unit construction lives in
`render_units.allium`. The windowing/scroll-anchor behavioural layer is
no longer Phoenix code; it is delegated to `react-virtuoso` (see
REQ-MLRU-015), so the corresponding `windowing.allium` was removed in
task 60410.

## File Layout

```
ui/src/
├── conversation/
│   └── renderUnits.ts           # buildRenderUnits + types
│      (joins existing atom.ts, ConversationStore.ts, etc. as the
│       projection from atom state to render shape)
│
├── components/
│   ├── MessageList.tsx          # Builds units, renders single <Virtuoso>
│   ├── StreamingMessage.tsx     # Subscribes to buffer atom internally
│   └── MessageList.test.tsx     # Unit + dispatch + ack-identity tests
│
└── generated/
    └── (no changes — render units are a UI-only concept; no wire types)
```

The following files existed in earlier iterations and were removed in
task 60410 when virtuoso took over windowing:

- `ui/src/hooks/useBottomAnchoredWindow.ts` (+ test)
- `ui/src/hooks/useUnitHeightObserver.ts`
- `ui/src/conversation/unitHeightCache.ts` (+ test)

## Type Shapes

```ts
// ui/src/conversation/renderUnits.ts

import type { Message, QueuedMessage, ConversationState } from '../api';

export type AwaitingSubAgentsState = Extract<
  ConversationState,
  { type: 'awaiting_sub_agents' }
>;

/** Units that live in the virtualized historical list. */
export type HistoricalUnit =
  | { kind: 'user';         key: string; message: Message }
  | { kind: 'skill';        key: string; message: Message }
  | { kind: 'agent_turn';
      key: string;
      agent: Message;
      toolResultsByUseId: ReadonlyMap<string, Message>;
      isFirstInTurn: boolean }
  | { kind: 'system';       key: string; message: Message }
  | { kind: 'pending_user'; key: string; message: QueuedMessage };

/** Units pinned to the tail; never collapsed by the window. */
export type TailUnit =
  | { kind: 'sub_agent_status'; key: string; state: AwaitingSubAgentsState }
  | { kind: 'streaming_agent';  key: string };

export type RenderUnit = HistoricalUnit | TailUnit;

export interface RenderUnits {
  historicalUnits: HistoricalUnit[];
  tailUnits: TailUnit[];
}

export interface BuildInputs {
  messages: Message[];
  pendingMessages: QueuedMessage[];
  convState: ConversationState;
  /** Tag for the active streaming session (null when no buffer). Built
   *  by the caller from `useStreamingRequestId(slug)`; the key is the
   *  server-allocated `request_id` for the in-flight LLM dispatch and
   *  also becomes the finalized agent message's `message_id`, so the
   *  streaming `TailUnit` and the eventual `agent_turn` `HistoricalUnit`
   *  share a key by construction — symmetric to pending_user → user. */
  streamingHandle: { key: string } | null;
}

export function buildRenderUnits(inputs: BuildInputs): RenderUnits;
```

### Key naming convention

- `user` / `agent_turn` / `system` / `skill` units: `key = message.message_id`
- `pending_user`: `key = queuedMessage.localId` — by convention the server
  echoes the message back with `message_id = localId`, so the same render-
  unit key persists through the pending → sent transition (the unit's
  *kind* changes from `pending_user` to `user`, but React's reconciler
  treats it as an in-place update on a single keyed node — no
  cross-region promotion, no scroll compensation needed).
- `sub_agent_status`: `key = 'sub-agent-status'` (singleton)
- `streaming_agent`: `key = streamingBuffer.requestId` — the server-allocated
  `request_id` for the active LLM dispatch. Stable across all tokens in
  the session (every Token SSE event carries it) AND becomes the
  finalized agent message's `message_id` on persistence, so the
  streaming `TailUnit` and the eventual `agent_turn` `HistoricalUnit`
  share a render-unit key. The streaming → sent transition is therefore
  an in-place keyed update on a single render unit — symmetric to
  pending_user → user (REQ-MLRU-001). No cross-region key swap, no
  imperative scroll compensation needed for the transition.

## Construction Algorithm

`buildRenderUnits` is a single-pass walk of `messages`:

```ts
export function buildRenderUnits(inputs: BuildInputs): RenderUnits {
  const { messages, pendingMessages, convState, streamingHandle } = inputs;
  const historicalUnits: HistoricalUnit[] = [];

  let i = 0;
  let inAgentRun = false;

  while (i < messages.length) {
    const msg = messages[i];
    const type = msg.message_type || msg.type;

    if (type === 'user') {
      historicalUnits.push({ kind: 'user', key: msg.message_id, message: msg });
      inAgentRun = false;
      i++;
    } else if (type === 'skill') {
      historicalUnits.push({ kind: 'skill', key: msg.message_id, message: msg });
      inAgentRun = false;
      i++;
    } else if (type === 'agent') {
      const { unit, consumed } = buildAgentTurn(messages, i, !inAgentRun);
      historicalUnits.push(unit);
      inAgentRun = true;
      i += consumed;
    } else if (type === 'system') {
      const text = (msg.content as { text?: string })?.text;
      if (text) {
        historicalUnits.push({ kind: 'system', key: msg.message_id, message: msg });
      } else {
        console.debug('[renderUnits] skipped empty system', {
          message_id: msg.message_id, reason: 'empty_system',
        });
      }
      // system messages do not break agent runs
      i++;
    } else if (type === 'tool') {
      // Orphan tool result (no preceding agent in the same run). Skip + log.
      console.debug('[renderUnits] skipped orphan tool', {
        message_id: msg.message_id, reason: 'orphan_tool',
      });
      i++;
    } else {
      console.debug('[renderUnits] skipped unknown type', {
        message_id: msg.message_id, message_type: type, reason: 'unknown_type',
      });
      i++;
    }
  }

  // Pending user messages append to the END of historicalUnits, sharing
  // the eventual `user` unit's key (localId == message_id at ack time).
  // This keeps the pending → sent transition an in-place keyed update
  // rather than a cross-region promotion from tailUnits to historicalUnits.
  for (const q of pendingMessages) {
    historicalUnits.push({ kind: 'pending_user', key: q.localId, message: q });
  }

  const tailUnits: TailUnit[] = [];

  if (convState.type === 'awaiting_sub_agents') {
    tailUnits.push({
      kind: 'sub_agent_status',
      key: 'sub-agent-status',
      state: convState,
    });
  }

  if (streamingHandle !== null) {
    tailUnits.push({
      kind: 'streaming_agent',
      key: streamingHandle.key,
    });
  }

  return { historicalUnits, tailUnits };
}

function buildAgentTurn(
  messages: Message[],
  startIdx: number,
  isFirstInTurn: boolean,
): { unit: HistoricalUnit; consumed: number } {
  const agent = messages[startIdx];
  const toolResultsByUseId = new Map<string, Message>();
  let j = startIdx + 1;
  while (j < messages.length) {
    const next = messages[j];
    const t = next.message_type || next.type;
    if (t !== 'tool') break;
    const toolUseId = (next.content as { tool_use_id?: string })?.tool_use_id;
    if (toolUseId) {
      toolResultsByUseId.set(toolUseId, next);
    } else {
      console.debug('[renderUnits] tool result missing tool_use_id', {
        message_id: next.message_id,
      });
    }
    j++;
  }
  return {
    unit: {
      kind: 'agent_turn',
      key: agent.message_id,
      agent,
      toolResultsByUseId,
      isFirstInTurn,
    },
    consumed: j - startIdx,
  };
}
```

The function is pure, takes only data, has no DOM dependencies, and is
fully unit-testable. `streamingHandle` is derived at the call site
via `useStreamingRequestId(slug)`: when streaming is active the hook
returns the buffer's `requestId` (the server-allocated `request_id`
stable across all tokens in the session AND identical to the eventual
finalized `AssistantMessage.message_id`), which MessageList wraps into
`{ key: requestId }`. The streaming → sent transition therefore
preserves render-unit identity by construction.

## Virtualization (REQ-MLRU-015)

MessageList renders a single `<Virtuoso>` instance from `react-virtuoso`
with the concatenation `[...historicalUnits, ...tailUnits]` as its
`data` prop. Virtuoso owns: windowing (which items are in DOM at any
time), scroll-anchor compensation (preserving viewport when items mount
above the viewport), and item-height measurement. Auto-follow (pin-to-
bottom behavior) is NOT delegated to Virtuoso; `followOutput={false}`
disables Virtuoso's built-in auto-scroll, and the `totalListHeightChanged`
callback is the sole auto-scroll mechanism (see REQ-MLRU-014).

Key configuration:

```tsx
<Virtuoso
  key={conversationId ?? '__empty__'}
  ref={virtuosoRef}
  data={allUnits}
  itemContent={(_, unit) => (
    <div className="virtuoso-row" data-render-unit-key={unit.key}>
      {renderUnit(unit, slug, onOpenFile, onRetry, onCancelSteering)}
    </div>
  )}
  computeItemKey={(_, unit) => unit.key}
  followOutput={false}
  atBottomThreshold={100}
  atBottomStateChange={(atBottom) => setIsAtBottom(atBottom)}
  totalListHeightChanged={handleTotalListHeightChanged}
  initialTopMostItemIndex={allUnits.length > 0 ? allUnits.length - 1 : 0}
  alignToBottom
  increaseViewportBy={{ top: 600, bottom: 600 }}
  {...(SystemPromptHeaderSlot ? { components: { Header: SystemPromptHeaderSlot } } : {})}
  className="message-virtuoso"
/>
```

Rationale for each non-default knob:

- `key={conversationId}` — forces a fresh Virtuoso instance per
  conversation. Virtuoso's measurement cache and pin state are owned
  per-instance; remount guarantees a clean bottom-pinned landing on
  every switch without leakage from the prior conversation. Cost: a
  re-measure on return visits, acceptable given the sub-second visit
  cadence and small typical conversation size.
- `followOutput={false}` — disables ALL of Virtuoso's built-in auto-scroll
  mechanisms (both the totalCount-based followOutput and the size-increase
  handler). Auto-follow is handled solely by the `totalListHeightChanged`
  callback. This is necessary because Virtuoso's built-in mechanisms don't
  handle streaming token growth correctly: the totalCount-based followOutput
  only fires on `data.length` changes (new items), not height-only changes
  (streaming token growth); and the size-increase handler misclassifies user
  scroll-up as content growth during streaming (its `notAtBottomBecause`
  priority order checks `scrollHeight` growth before scroll direction),
  yanking the user back to the bottom. The manual `totalListHeightChanged`
  callback uses the pre-growth scroll position (`oldFromBottom`) to
  distinguish "user was near the bottom" from "user scrolled up," which is
  correct during streaming. See REQ-MLRU-014 for the full rationale.
- `atBottomThreshold={100}` — matches the prior hand-rolled
  `scrollHeight - scrollTop - clientHeight <= 100` threshold so the
  pin/no-pin classification stays identical to user expectations
  established by the previous implementation.
- `initialTopMostItemIndex={allUnits.length - 1}` + `alignToBottom` —
  bottom-pinned mount (replaces REQ-MLRU-005). For empty data, index 0
  is a no-op default.
- `increaseViewportBy={{ top: 600, bottom: 600 }}` — overscan distance
  matching the prior 600-pixel sentinel rootMargin so the perceived
  smoothness during scrollback is the same.

The `data-render-unit-key` attribute on each item wrapper is preserved
for selectors used by tests and dev tools (it is also a guarantee in
REQ-MLRU-001 that one DOM node per key persists through pending → sent
acknowledgement).

## Jump-to-Newest Button

Visibility is driven by Virtuoso's `atBottomStateChange` callback:

```tsx
{!isEmpty && !isAtBottom && (
  <button className="jump-to-newest" onClick={scrollToNewest}>
    ↓ New messages
  </button>
)}
```

Click calls the imperative Virtuoso ref:

```tsx
virtuosoRef.current?.scrollToIndex({
  index: 'LAST',
  align: 'end',
  behavior: 'auto',
});
```

`behavior: 'auto'` is an instant snap (no animated scroll), matching
the prior `scrollTop = scrollHeight` pattern. After the scroll settles,
Virtuoso fires `atBottomStateChange(true)` and the button is removed
by the conditional render.

## System Prompt as Virtuoso Header

When `systemPrompt` is non-empty, MessageList provides a Header slot to
Virtuoso:

```tsx
const SystemPromptHeaderSlot = useMemo(() => {
  if (!systemPrompt) return undefined;
  const Header = () => (
    <SystemPromptHeader
      systemPrompt={systemPrompt}
      expanded={systemPromptExpanded}
      onToggle={toggleSystemPrompt}
    />
  );
  return Header;
}, [systemPrompt, systemPromptExpanded, toggleSystemPrompt]);
```

Virtuoso treats the Header as item 0; it scrolls with content and is
measured like any item. This means the system prompt scrolls off-screen
when the user reads down through the conversation, matching the prior
behavior where the prompt lived inside the same scrolling container.

The Header prop is omitted (rather than passed `undefined`) when there
is no prompt — `exactOptionalPropertyTypes: true` in tsconfig rejects
explicit-undefined component prop values, so MessageList spreads
conditionally via `{...(SystemPromptHeaderSlot ? { components: { Header: SystemPromptHeaderSlot } } : {})}`.

## Saved-Scroll Anchor (REMOVED)

REQ-MLRU-009 was deprecated and the anchor-capture / restore /
ack-DOM-snapshot machinery removed. No localStorage key is read or
written. Mount lands pinned to the bottom (REQ-MLRU-015); per
REQ-MLRU-001, pending → sent acknowledgement is a keyed in-place
update on a single render unit, so no ack-time scroll compensation
is needed.

## Streaming Subscription

Today's `MessageList` accepts `streamingBuffer?: StreamingBuffer` as a
prop. Even though `MessageListBody` is memoized, the parent `MessageList`
re-renders on every token because its prop changed.

The new path:

1. `MessageList` no longer accepts `streamingBuffer` as a prop.
2. `<StreamingMessage />` reads the buffer via
   `useAtomValue(streamingBufferAtom)` (or `useSyncExternalStore` if the
   atom is not a Jotai atom).
3. `buildRenderUnits` accepts `isStreaming: boolean` derived at the
   `MessageList` call site as `convState.type === 'llm_requesting'`
   (or whatever the streaming-active predicate is — see
   `specs/conversation_atom/conversation_atom.allium`'s
   `StreamingBufferPhaseCoupled` invariant).
4. The presence/absence of the `streaming_agent` tail unit is a coarse
   event (changes on streaming-start and streaming-stop), not per-token.

**Prerequisite verification:** the streaming buffer must be on an
externally-subscribable store. The audit indicated `atom.ts` is the
source; if the atom shape does not support selective subscription, a
small extraction lands as a precursor commit in the same PR. This is
explicitly tracked as part of this task's scope, not deferred.

## Capability-Gap Logging

Every skip path in `buildRenderUnits` emits a `console.debug`. The
structured shape is:

```ts
console.debug('[renderUnits] ' + summary, {
  message_id: string,
  message_type?: string,
  reason: 'empty_system' | 'orphan_tool' | 'unknown_type' | 'missing_tool_use_id',
  /* optional reason-specific fields */
});
```

This is sufficient for the next developer adding a new message type to
notice their messages disappearing.

## Testing Strategy

**Unit-construction tests** (`renderUnits.test.ts`):

1. Empty messages → empty historical, no tail units beyond pending +
   sub-agent + streaming
2. User → user unit with `isFirstInTurn = true` for the next agent
3. Skill → skill unit, breaks agent run
4. Single agent message → single `agent_turn` with empty
   `toolResultsByUseId` and `isFirstInTurn = true`
5. Agent with N tool messages → single `agent_turn` with N entries in the
   map, `consumed = N + 1` messages
6. Agent + tool + agent + tool → two `agent_turn` units, second has
   `isFirstInTurn = false`
7. Agent + tool + user → user breaks the run; subsequent agent is
   first-in-turn
8. Orphan tool (first message is tool) → skipped, debug logged
9. Tool with no `tool_use_id` → still mapped under empty string? or
   skipped? **Open question — resolve before implementing.** Recommended:
   skip + log, since a result with no use_id cannot pair structurally
10. Empty system → skipped, debug logged
11. System with text → emitted
12. Unknown type → skipped, debug logged
13. Pending messages → appended to tail units in order
14. `convState.type === 'awaiting_sub_agents'` → tail unit emitted with
    that state
15. `isStreaming = true` → `streaming_agent` tail unit emitted

**Virtuoso behavioral tests** are out of scope for unit-test coverage:
react-virtuoso requires real DOM measurement that happy-dom does not
provide. The unit test file mocks `react-virtuoso` as a passthrough
(renders the Header + all items) so React reconciler behavior is
testable in isolation; real windowing and scroll-anchor behavior is
verified by an in-browser smoke pass (see task 60410 acceptance
criteria).

**Integration tests** (`MessageList.test.tsx`):

1. **REQ-MLRU-012 regression:** 1 user + 1 agent (20 tool_use blocks) +
   20 tool messages → `agent_turn` is in rendered DOM
2. Pending → sent acknowledgement: render with a `pending_user` unit,
   rerender with the same `localId` echoed as a `user` message →
   render-unit key persists, same DOM node identity; no extra commits
3. 100-message render smoke — MessageList builds + dispatches units
   without throwing

The streaming-isolation perf invariant (REQ-MLRU-010) is verified by
`MessageList.perf-isolation.test.tsx` against the same passthrough
virtuoso mock — virtuoso's internal scheduling is irrelevant to the
streaming-isolation commit-count assertion.

## Performance Validation

Verified empirically by exercising the in-browser smoke (conversation
switch, scroll-back through a 500-message fixture, streaming token
arrival pinned and scrolled-up). Quantitative profiling (`browser_profile
conversation-load`) is available if a regression is suspected. Virtuoso
is the same library shipped by Slack, Discord, Linear, and Notion for
the same use case; defaults are tuned for chat-style payloads.

## Open Questions Resolved by This Spec

- **Pending/sent split (PR #152 hotfix):** REQ-MLRU-001 puts
  `pending_user` in `HistoricalUnit` sharing its eventual `user` unit's
  key (localId == message_id at ack), so pending → sent is an in-place
  keyed update — no cross-region promotion, no ack scroll compensation.
- **Sub-agent and streaming positioning:** REQ-MLRU-004 keeps them as
  `TailUnit`s (ephemeral, no ack lifecycle).
- **Spacer over/under-allocation:** Resolved structurally by REQ-MLRU-015
  — Virtuoso owns measurement; Phoenix has no spacer DOM elements.
- **Streaming and unified-list tension:** REQ-MLRU-010 typifies streaming
  as a tail unit whose leaf component owns the subscription.
- **Hand-rolled scroll-anchor compensation drift (PR #161/162/163):**
  Resolved structurally by REQ-MLRU-015 — Virtuoso owns the anchor
  contract.

## Open Question Carried Forward

- **Tool result without `tool_use_id`:** the construction algorithm
  currently logs and continues without adding to the map. This means the
  result is silently dropped from the rendered agent_turn. Acceptable if
  the backend invariant is "every tool result has a tool_use_id" (which
  is true per the SSE wire spec); raise an error in dev if observed.
  Confirm with the user before implementing if a stricter behavior is
  preferred.
