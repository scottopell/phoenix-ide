import { useCallback, useContext, useSyncExternalStore, useRef, type Dispatch } from 'react';
import type { ConversationAtom, SSEAction, StreamingBuffer } from './atom';
import type { Conversation } from '../api';
import { ConversationContext } from './ConversationContext';
import { conversationListsEqual } from '../utils/conversationDiff';
import { isAgentWorking } from '../utils';

function useConversationStore() {
  const store = useContext(ConversationContext);
  if (!store) throw new Error('useConversationAtom must be used within ConversationProvider');
  return store;
}

/**
 * Returns [atom, dispatch] for the given conversation slug.
 *
 * Subscribes only to this slug's atom via the external store — updates to
 * other conversation slugs do not cause this hook to re-render.
 */
export function useConversationAtom(slug: string): [ConversationAtom, Dispatch<SSEAction>] {
  const store = useConversationStore();

  const subscribe = useCallback(
    (listener: () => void) => store.subscribe(slug, listener),
    [store, slug],
  );
  const getSnapshot = useCallback(
    () => store.getSnapshot(slug),
    [store, slug],
  );

  const atom = useSyncExternalStore(subscribe, getSnapshot);

  const dispatch = useCallback(
    (action: SSEAction) => store.dispatch(slug, action),
    [store, slug],
  );

  return [atom, dispatch];
}

/** Derived selectors to avoid passing the raw atom to child components. */
export function useConversationSelectors(slug: string) {
  const [atom, dispatch] = useConversationAtom(slug);

  const currentTool =
    atom.phase.type === 'tool_executing' || atom.phase.type === 'cancelling_tool'
      ? atom.phase.current_tool
      : null;

  return {
    atom,
    dispatch,
    isAgentWorking: isAgentWorking(atom.phase),
    currentTool,
    streamingText: atom.streamingBuffer?.text ?? null,
    breadcrumbs: atom.breadcrumbs,
  };
}

/**
 * Returns the conversation snapshot for `slug`, or null if no atom for
 * that slug has been observed yet (or the slug is null).
 *
 * Reads `atom.conversation` only — by design, this hook does NOT
 * re-render on `sse_token` (which churns `streamingBuffer`) or on most
 * other live mutations. `useSyncExternalStore` compares snapshots with
 * `Object.is`, so returning the same `Conversation` reference twice
 * elides the render.
 *
 * Replaces the old per-field `useConversationCwd` bridge from task
 * 08612: the store is now the single source of truth for the
 * conversation row, so a per-field selector is no longer needed.
 * Consumers read `useConversationSnapshot(slug)?.cwd` and the same
 * principle applies to every other field.
 */
export function useConversationSnapshot(slug: string | null): Conversation | null {
  const store = useConversationStore();

  const subscribe = useCallback(
    (listener: () => void) => (slug ? store.subscribe(slug, listener) : () => {}),
    [store, slug],
  );
  const getSnapshot = useCallback(
    () => (slug ? store.getSnapshot(slug).conversation ?? null : null),
    [store, slug],
  );

  return useSyncExternalStore(subscribe, getSnapshot);
}

/**
 * Returns `{ active, archived }` arrays of every Conversation snapshot
 * the store currently holds, sorted by `updated_at DESC`. Reference-
 * stable across re-renders unless `(id, updated_at)` for some row
 * actually changes — a polling tick that returns equivalent rows
 * doesn't churn the array reference.
 *
 * This is the sidebar's read path post-08684. The previous per-component
 * `Conversation[]` state is gone; both `DesktopLayout` and
 * `ConversationListPage` consume this hook.
 */
export function useConversationsList(): {
  active: readonly Conversation[];
  archived: readonly Conversation[];
} {
  const store = useConversationStore();

  // Cache the last-derived value across calls. `useSyncExternalStore`
  // requires `getSnapshot` to return the same reference for unchanged
  // values — otherwise it would force a re-render on every dispatch
  // anywhere in the store. We compare per-row by (id, updated_at) and
  // reuse the previous arrays when the comparison matches.
  const lastRef = useRef<{
    active: readonly Conversation[];
    archived: readonly Conversation[];
  }>({ active: [], archived: [] });

  const subscribe = useCallback(
    (listener: () => void) => store.subscribeAny(listener),
    [store],
  );

  const getSnapshot = useCallback(() => {
    const all = store.listSnapshots();
    const nextActive: Conversation[] = [];
    const nextArchived: Conversation[] = [];
    for (const c of all) {
      if (c.archived) nextArchived.push(c);
      else nextActive.push(c);
    }
    nextActive.sort(byUpdatedAtDesc);
    nextArchived.sort(byUpdatedAtDesc);

    const prev = lastRef.current;
    const sameActive = conversationListsEqual(prev.active, nextActive);
    const sameArchived = conversationListsEqual(prev.archived, nextArchived);
    if (sameActive && sameArchived) return prev;

    const next = {
      active: sameActive ? prev.active : nextActive,
      archived: sameArchived ? prev.archived : nextArchived,
    };
    lastRef.current = next;
    return next;
  }, [store]);

  return useSyncExternalStore(subscribe, getSnapshot);
}

/**
 * Returns the streaming buffer for `slug`, or null when no buffer is
 * active. Reference-stable: the atom's `streamingBuffer` field is the
 * snapshot; `useSyncExternalStore` skips re-renders when the reference
 * is unchanged (atom mutations to other fields like `phase` or
 * `messages` don't cause this hook to re-render — only buffer
 * mutations do, which is exactly the per-token re-render the consuming
 * `<StreamingMessage>` leaf needs).
 *
 * This is the seam that lets `<MessageList>` stop receiving
 * `streamingBuffer` as a prop: the leaf subscribes directly, the
 * historical render tree no longer participates in per-token updates.
 */
export function useStreamingBuffer(slug: string): StreamingBuffer | null {
  const store = useConversationStore();

  const subscribe = useCallback(
    (listener: () => void) => store.subscribe(slug, listener),
    [store, slug],
  );
  const getSnapshot = useCallback(
    () => store.getSnapshot(slug).streamingBuffer ?? null,
    [store, slug],
  );

  return useSyncExternalStore(subscribe, getSnapshot);
}

/**
 * Returns the server-authoritative `phaseStateUpdatedAt` (unix ms) for
 * `slug`. Re-renders the consumer on every phase transition, since the
 * value changes wholesale per `StateChange` SSE event. Returned as a
 * number so `Object.is` comparison short-circuits when the timestamp
 * is unchanged.
 *
 * Used by the pending-assistant-bubble and any other leaf component
 * that needs to render a live elapsed-time counter without prop-
 * drilling through the full atom (REQ-WPV-001).
 */
export function usePhaseStateUpdatedAt(slug: string): number | null {
  const store = useConversationStore();
  const subscribe = useCallback(
    (listener: () => void) => store.subscribe(slug, listener),
    [store, slug],
  );
  const getSnapshot = useCallback(
    () => store.getSnapshot(slug).phaseStateUpdatedAt,
    [store, slug],
  );
  return useSyncExternalStore(subscribe, getSnapshot);
}

/**
 * Returns the streaming buffer's `requestId` for `slug`, or null when
 * no buffer is active. Re-renders the consumer on streaming-start,
 * streaming-end, AND streaming-restart (a new buffer with a fresh
 * `requestId`), because `Object.is` comparison on the returned string
 * distinguishes those transitions.
 *
 * `requestId` is the server-generated id stamped on every `Token` SSE
 * event AND on the eventual `AssistantMessage.message_id`. Using it as
 * the streaming render unit's key means the streaming → sent transition
 * preserves key identity — the streaming `TailUnit` and the finalized
 * `agent_turn` `HistoricalUnit` share a key, so virtuoso (and the
 * React reconciler in general) sees an in-place keyed update rather
 * than a cross-region key swap. Symmetric to REQ-MLRU-001's
 * pending_user → user pattern.
 *
 * The boolean "is streaming active right now" is just
 * `useStreamingRequestId(slug) !== null`.
 */
export function useStreamingRequestId(slug: string | undefined): string | null {
  const store = useConversationStore();

  const subscribe = useCallback(
    (listener: () => void) => (slug ? store.subscribe(slug, listener) : () => {}),
    [store, slug],
  );
  const getSnapshot = useCallback(
    () => (slug ? store.getSnapshot(slug).streamingBuffer?.requestId ?? null : null),
    [store, slug],
  );

  return useSyncExternalStore(subscribe, getSnapshot);
}

function byUpdatedAtDesc(a: Conversation, b: Conversation): number {
  // Lexicographic comparison on ISO timestamps is chronological. Newer
  // first — sidebar order.
  if (a.updated_at > b.updated_at) return -1;
  if (a.updated_at < b.updated_at) return 1;
  return 0;
}
