# MessageList Render Units — Design

This document describes the technical architecture implementing the
requirements in `specs/messagelist-render-units/requirements.md`. The
behavioural specifications live in `render_units.allium` (unit
construction) and `windowing.allium` (window lifecycle).

## File Layout

```
ui/src/
├── conversation/
│   ├── renderUnits.ts           # NEW. buildRenderUnits + types
│   ├── renderUnits.test.ts      # NEW. Unit-construction tests
│   ├── unitHeightCache.ts       # NEW. measured-height map + sessionStorage
│   └── unitHeightCache.test.ts  # NEW.
│   (joins existing atom.ts, ConversationStore.ts, etc. as the
│   projection from atom state to render shape)
│
├── hooks/
│   ├── useBottomAnchoredWindow.ts   # REWORKED. Takes HistoricalUnit[]
│   └── useUnitHeightObserver.ts     # NEW. ResizeObserver-per-unit wiring
│
├── components/
│   ├── MessageList.tsx          # REWORKED. Derives units, slices, dispatches
│   ├── MessageListBody.tsx      # SPLIT OUT. Pure (HistoricalUnit[], TailUnit[]) -> JSX
│   ├── StreamingMessage.tsx     # REWORKED. Subscribes to buffer atom internally
│   └── MessageList.test.tsx     # EXTENDED. Adds tool-result-heavy-tail regression
│
└── generated/
    └── (no changes — render units are a UI-only concept; no wire types)
```

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
  | { kind: 'user';   key: string; message: Message }
  | { kind: 'skill';  key: string; message: Message }
  | { kind: 'agent_turn';
      key: string;
      agent: Message;
      toolResultsByUseId: ReadonlyMap<string, Message>;
      isFirstInTurn: boolean }
  | { kind: 'system'; key: string; message: Message };

/** Units pinned to the tail; never collapsed by the window. */
export type TailUnit =
  | { kind: 'pending_user';     key: string; message: QueuedMessage }
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
   *  by the caller from `useStreamingStartedAt(slug)`; the key
   *  embeds startedAt so back-to-back sessions remount cleanly. */
  streamingHandle: { key: string } | null;
}

export function buildRenderUnits(inputs: BuildInputs): RenderUnits;
```

### Key naming convention

- `user` / `agent_turn` / `system` / `skill` units: `key = message.message_id`
- `pending_user`: `key = queuedMessage.localId`
- `sub_agent_status`: `key = 'sub-agent-status'` (singleton)
- `streaming_agent`: `key = 'streaming-${conversationId}-${startedAt}'` —
  the `startedAt` from `StreamingBuffer` makes the key stable across token
  arrivals but unique per streaming session. The atom owns the buffer; the
  unit just needs to be identity-stable so React preserves the leaf state
  across re-derivations.

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

  const tailUnits: TailUnit[] = [];

  for (const q of pendingMessages) {
    tailUnits.push({ kind: 'pending_user', key: q.localId, message: q });
  }

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
via `useStreamingStartedAt(slug)`: when streaming is active the hook
returns the buffer's `startedAt` (a number stable across tokens but
unique per session), which MessageList wraps into
`{ key: \`streaming-${slug}-${startedAt}\` }`.

## Window Hook

```ts
// ui/src/hooks/useBottomAnchoredWindow.ts

export interface SavedScrollAnchor {
  topVisibleUnitKey: string;
  offsetWithinUnit: number;
  /** Number of historical units at save time. Restore compares this
   *  to current `historicalUnits.length` to detect "messages arrived
   *  while away" and surface the ↓ New messages button. Optional for
   *  forward-compat with anchors written by older app builds. */
  unitCountAtSave?: number;
}

export interface UseWindowInputs {
  historicalUnits: HistoricalUnit[];
  conversationId: string | undefined;
  scrollRootRef: React.RefObject<HTMLElement | null>;
  savedAnchor?: SavedScrollAnchor | null;
  heightCache?: UnitHeightCache | null;
}

export interface UseWindowOutputs {
  firstRenderedUnitIndex: number;
  spacerHeight: number;
  /** Callback ref — when the sentinel DOM node mounts (which may
   *  happen on a later render than the first, e.g. an empty
   *  conversation that grows past INITIAL_WINDOW), this triggers the
   *  IntersectionObserver setup effect via a state-backed ref. A
   *  RefObject would not, because ref-mutation doesn't trigger
   *  re-runs of useEffect. */
  topSentinelRef: (node: HTMLDivElement | null) => void;
}

export function useBottomAnchoredWindow(inputs: UseWindowInputs): UseWindowOutputs;
```

Constants:

```ts
export const INITIAL_WINDOW = 12;
export const EXPAND_BATCH = 12;
export const SENTINEL_ROOT_MARGIN = '600px 0px 0px 0px';
export const RESTORE_OVERSCAN = 4;

export const KIND_ESTIMATES: Record<HistoricalUnit['kind'], number> = {
  user: 100,
  skill: 80,
  agent_turn: 400,
  system: 100,
};
```

Internal mechanics:

1. `firstRenderedUnitIndex` is React state. Initial value is computed once
   per conversation from `(historicalUnits.length, savedAnchor)`.
2. A `useEffect` attaches an `IntersectionObserver` to `topSentinelRef`
   with `root: scrollRootRef.current` and `rootMargin: SENTINEL_ROOT_MARGIN`.
   When the sentinel intersects, the effect decreases
   `firstRenderedUnitIndex` by `EXPAND_BATCH` (clamped at 0).
3. Before the state decrement, the effect captures
   `scrollRootRef.current.scrollHeight` into `prevScrollHeightRef`.
4. A `useLayoutEffect` keyed on `firstRenderedUnitIndex` consumes
   `prevScrollHeightRef` and adjusts `scrollTop` by the delta.
5. `spacerHeight` is computed via `useMemo` over the prefix slice and the
   `heightCache`. When the cache emits a change event, the memo
   reconciles and the spacer re-renders.

The hook returns `topSentinelRef` so the component places the sentinel
correctly in the DOM:

```tsx
<div className="message-collapsed-spacer" style={{ height: spacerHeight }} />
<div ref={topSentinelRef} aria-hidden />
{historicalUnits.slice(firstRenderedUnitIndex).map(renderUnit)}
{tailUnits.map(renderTailUnit)}
```

## Height Cache

```ts
// ui/src/conversation/unitHeightCache.ts

const STORAGE_PREFIX = 'phoenix:hcache:';

export class UnitHeightCache {
  constructor(private readonly conversationId: string | undefined) {
    this.heights = new Map();
    this.listeners = new Set();
    this.hydrateFromStorage();
  }

  set(key: string, height: number): void;
  get(key: string): number | undefined;
  subscribe(listener: () => void): () => void;
  clear(): void;

  /** Called by conversation-delete cascade. */
  static clearConversation(conversationId: string): void;
}

export function useUnitHeightCache(conversationId: string | undefined): UnitHeightCache;
```

Reads are O(1) from the Map; the `sessionStorage` mirror is write-through
and hydrated synchronously on construction. Writes are debounced (16ms
trailing) to coalesce ResizeObserver bursts during scroll. Subscribers are
notified after each successful write — the window hook subscribes so that
spacer height updates land in a re-render.

## Unit Height Observer

```ts
// ui/src/hooks/useUnitHeightObserver.ts

/**
 * Returns a callback ref for each unit element. The callback attaches a
 * ResizeObserver and writes measured heights into the cache keyed by
 * unit.key.
 */
export function useUnitHeightObserver(cache: UnitHeightCache): (
  unit: HistoricalUnit,
) => (el: HTMLElement | null) => void;
```

Used in `MessageListBody`:

```tsx
const observe = useUnitHeightObserver(heightCache);

return (
  <>
    {renderedUnits.map((unit) => (
      <div key={unit.key} ref={observe(unit)}>
        {renderUnit(unit)}
      </div>
    ))}
  </>
);
```

The ref callback caches per-unit-key observer instances to avoid
re-creating observers on every render.

## Saved-Scroll Anchor

```ts
// inside MessageList.tsx

interface SavedAnchorStorage {
  read(conversationId: string): SavedScrollAnchor | null;
  write(conversationId: string, anchor: SavedScrollAnchor): void;
}

const STORAGE_PREFIX = 'phoenix:msglist:anchor:';
```

**Write path:** on `document.visibilitychange === 'hidden'` and on
component unmount:

1. Walk the rendered unit DOM nodes (via a ref-map keyed by `unit.key`)
2. Find the first node whose `offsetTop >= scrollRoot.scrollTop`
3. Compute `offsetWithinUnit = scrollRoot.scrollTop - node.offsetTop`
4. Persist `{ topVisibleUnitKey: unit.key, offsetWithinUnit }`

**Read path:** on first render with a non-empty `historicalUnits[]`:

1. Look up the saved anchor for the conversation id
2. Find `foundIndex = historicalUnits.findIndex(u => u.key === anchor.topVisibleUnitKey)`
3. If `foundIndex < 0`: fall back to bottom-pin
4. Otherwise: set `firstRenderedUnitIndex = max(0, foundIndex - RESTORE_OVERSCAN)`
5. In a layout effect after first paint: locate the DOM node for that
   unit (via the ref-map), set
   `scrollRoot.scrollTop = node.offsetTop + anchor.offsetWithinUnit`

If `sessionStorage` height cache (REQ-MLRU-013) has measured heights, the
spacer height for the prefix is exact, so the unit's `offsetTop` is
correct on first paint without an extra layout pass.

## Migration

The existing `localStorage` key `${SCROLL_KEY_PREFIX}{conversationId}`
stores a number (scrollTop). The new anchor key is
`phoenix:msglist:anchor:{conversationId}` storing a JSON
`SavedScrollAnchor`. The old key is **not** migrated — one visit's worth of
restore-to-bottom regression is acceptable, and conflating old/new shapes
behind a single key invites parsing ambiguity. The old key is deleted on
the first successful anchor write per conversation.

The `${MSGCOUNT_KEY_PREFIX}` companion key is no longer needed and can be
deleted in the same write.

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

**Window-hook tests** (mocked DOM):

1. Initial mount with no anchor → `firstRenderedUnitIndex = max(0, len - 12)`
2. Initial mount with anchor found → index = `foundIndex - 4`, clamped at 0
3. Initial mount with anchor not found → falls back to default
4. Sentinel intersection → `firstRenderedUnitIndex` decreases by 12
5. Sentinel intersection at index 0 → no-op
6. Scroll compensation: scrollHeight increases by Δ → scrollTop increases
   by Δ

**Integration tests** (`MessageList.test.tsx`):

1. **REQ-MLRU-012 regression:** 1 user + 1 agent (20 tool_use blocks) +
   20 tool messages → `agent_turn` is in initial rendered DOM
2. Saved anchor restore: write anchor, remount → scrollTop matches saved
3. Saved anchor with missing key → bottom-pinned mount
4. Streaming start → `<StreamingMessage />` renders below tail; parent
   `MessageList` does not re-render on simulated token bursts
5. Streaming complete → `streaming_agent` disappears, `agent_turn`
   appears, single commit

## Performance Validation

After implementation, validate with `browser_profile conversation-load`:

1. Baseline: current main branch with bottom-anchored window
2. Refactor branch: render-units + sentinel + measured spacer + atom-leaf
   streaming
3. Compare: `react_commit_ms`, `long_task_count`, `script_ms`,
   `total_blocking_time`

Expected: equal or better commit time (the slice-only render path removes
the Set lookup and the inline filter); script_ms should improve in the
streaming case (parent no longer re-renders).

If `script_ms` regresses materially, the most likely culprits are:
- ResizeObserver too noisy (mitigate: debounce writes further)
- Cache subscriber re-rendering parent (mitigate: scope subscription to
  the spacer component only via `useSyncExternalStore`)
- IntersectionObserver firing repeatedly during fast scroll (mitigate:
  disconnect during in-flight expansion, re-attach after commit)

Re-evaluate task 65003 against the new model after this work lands; some
of its tuning targets may be obsoleted by the sentinel + atom-leaf
streaming.

## Open Questions Resolved by This Spec

- **Pending and sub-agent ambiguity:** REQ-MLRU-004 makes them
  `TailUnit`s, structurally distinct from historical units.
- **Saved-scroll estimate fragility:** REQ-MLRU-009 anchors by unit key,
  not by pixel division.
- **Spacer over/under-allocation:** REQ-MLRU-008 measures per unit with
  per-kind fallback.
- **Streaming and unified-list tension:** REQ-MLRU-010 typifies streaming
  as a tail unit whose leaf component owns the subscription.

## Open Question Carried Forward

- **Tool result without `tool_use_id`:** the construction algorithm
  currently logs and continues without adding to the map. This means the
  result is silently dropped from the rendered agent_turn. Acceptable if
  the backend invariant is "every tool result has a tool_use_id" (which
  is true per the SSE wire spec); raise an error in dev if observed.
  Confirm with the user before implementing if a stricter behavior is
  preferred.
