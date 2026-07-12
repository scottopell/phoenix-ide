import { useCallback, useContext, useEffect, useSyncExternalStore, useRef, type MutableRefObject, type Dispatch } from 'react';
import type { ConversationAtom, SSEAction, StreamingBuffer } from './atom';
import type { Conversation } from '../api';
import type { WorkScopeInventory } from '../generated/sse';
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

/**
 * The fields `<ConversationPage>` renders from. Excludes the two
 * highest-frequency atom fields (`streamingBuffer`, `lastSseEventAt`) so
 * their churn does not re-render the page. The render-isolation contract
 * — why those are excluded and which leaf subscribers consume them — is
 * specified in specs/conversation_atom/conversation_atom.allium
 * (Render Subscription Isolation) and witnessed by
 * useConversationView.perf-isolation.test.tsx.
 */
export type ConversationPageView = Pick<
  ConversationAtom,
  | 'conversationId'
  | 'conversation'
  | 'phase'
  | 'messages'
  | 'contextWindow'
  | 'systemPrompt'
  | 'uiError'
  | 'toolExecutingStartedAt'
  | 'phaseStateUpdatedAt'
  | 'firstByteRequestId'
  | 'turnRetryContext'
  | 'transcriptGeneration'
>;

const PAGE_VIEW_KEYS: readonly (keyof ConversationPageView)[] = [
  'conversationId',
  'conversation',
  'phase',
  'messages',
  'contextWindow',
  'systemPrompt',
  'uiError',
  'toolExecutingStartedAt',
  'phaseStateUpdatedAt',
  'firstByteRequestId',
  'turnRetryContext',
  'transcriptGeneration',
];

function pageViewsEqual(a: ConversationPageView, b: ConversationPageView): boolean {
  // Each field is reference-stable in the reducer when unchanged (every
  // case spreads `...a` and replaces only the fields it mutates), so
  // per-field `Object.is` is sufficient — no deep compare needed.
  for (const k of PAGE_VIEW_KEYS) {
    if (!Object.is(a[k], b[k])) return false;
  }
  return true;
}

/**
 * Like {@link useConversationAtom}, but the subscriber only re-renders when
 * a field the page actually renders from changes — NOT on `sse_token`
 * (streaming buffer) or `sse_event_observed` (heartbeat clock).
 *
 * `getSnapshot` rebuilds the view object on each call but returns the
 * previously-cached reference when every selected field is unchanged, so
 * `useSyncExternalStore`'s `Object.is` check elides the render. Same
 * caching contract as {@link useConversationsList}.
 */
export function useConversationView(
  slug: string,
): [ConversationPageView, Dispatch<SSEAction>] {
  const store = useConversationStore();
  const lastRef = useRef<ConversationPageView | null>(null);

  const subscribe = useCallback(
    (listener: () => void) => store.subscribe(slug, listener),
    [store, slug],
  );
  const getSnapshot = useCallback(() => {
    const a = store.getSnapshot(slug);
    const next: ConversationPageView = {
      conversationId: a.conversationId,
      conversation: a.conversation,
      phase: a.phase,
      messages: a.messages,
      contextWindow: a.contextWindow,
      systemPrompt: a.systemPrompt,
      uiError: a.uiError,
      toolExecutingStartedAt: a.toolExecutingStartedAt,
      phaseStateUpdatedAt: a.phaseStateUpdatedAt,
      firstByteRequestId: a.firstByteRequestId,
      turnRetryContext: a.turnRetryContext,
      transcriptGeneration: a.transcriptGeneration,
    };
    const prev = lastRef.current;
    if (prev && pageViewsEqual(prev, next)) return prev;
    lastRef.current = next;
    return next;
  }, [store, slug]);

  const atom = useSyncExternalStore(subscribe, getSnapshot);

  const dispatch = useCallback(
    (action: SSEAction) => store.dispatch(slug, action),
    [store, slug],
  );

  return [atom, dispatch];
}

/**
 * Subscribes to just the heartbeat-watchdog clock (`lastSseEventAt`) for
 * `slug`. Consumed below the page boundary (by `<ConnectedStateBar>`) so
 * the per-event bump re-renders only the StateBar, not the whole page.
 * The value is a primitive, so `Object.is` makes the snapshot trivially
 * stable between equal observations.
 */
export function useConversationEventCursorRef(slug: string): MutableRefObject<number> {
  const store = useConversationStore();
  const ref = useRef(store.getSnapshot(slug).lastAppliedEventSeq);

  useEffect(() => {
    ref.current = store.getSnapshot(slug).lastAppliedEventSeq;
    return store.subscribe(slug, () => {
      ref.current = store.getSnapshot(slug).lastAppliedEventSeq;
    });
  }, [store, slug]);

  return ref;
}

export function useLastSseEventAt(slug: string): number {
  const store = useConversationStore();

  const subscribe = useCallback(
    (listener: () => void) => store.subscribe(slug, listener),
    [store, slug],
  );
  const getSnapshot = useCallback(
    () => store.getSnapshot(slug).lastSseEventAt,
    [store, slug],
  );

  return useSyncExternalStore(subscribe, getSnapshot);
}

/**
 * Like {@link useLastSseEventAt}, but writes the heartbeat clock into a ref
 * instead of returning it as a render-driving value. The heartbeat bumps on
 * every token AND every `ping`; a value subscription re-renders its host on
 * each bump. The StateBar watchdog only samples this clock on its 1s
 * interval, so reading it from a ref lets the StateBar subtree skip the
 * per-event re-render entirely (the host never re-renders on the bump).
 * See the Render Subscription Isolation section in
 * specs/conversation_atom/conversation_atom.allium.
 */
export function useLastSseEventAtRef(slug: string): MutableRefObject<number> {
  const store = useConversationStore();
  const ref = useRef<number>(store.getSnapshot(slug).lastSseEventAt);
  useEffect(() => {
    const update = () => {
      ref.current = store.getSnapshot(slug).lastSseEventAt;
    };
    // Catch any bump between the initial render and this effect's commit,
    // then keep the ref current for every subsequent bump — no re-render.
    update();
    return store.subscribe(slug, update);
  }, [store, slug]);
  return ref;
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
 * Subscribes to just the work-scope inventory (`workScope`) for `slug`
 * (REQ-WSUI-010). Consumed by the `WorkScopeSection` in the left panel so a
 * resource-state push re-renders only the section, not the transcript. The reducer replaces
 * `workScope` by reference on each `sse_work_scope_update`, so `Object.is`
 * elides the render when nothing changed.
 */
export function useWorkScope(slug: string | null): WorkScopeInventory | null {
  const store = useConversationStore();

  const subscribe = useCallback(
    (listener: () => void) => (slug ? store.subscribe(slug, listener) : () => {}),
    [store, slug],
  );
  const getSnapshot = useCallback(
    () => (slug ? store.getSnapshot(slug).workScope : null),
    [store, slug],
  );

  return useSyncExternalStore(subscribe, getSnapshot);
}

/**
 * Returns `{ active, archived }` arrays of the top-level Conversation
 * snapshots the store currently holds, sorted by `updated_at DESC`.
 * Reference-stable across re-renders unless `(id, updated_at)` for some
 * row actually changes — a polling tick that returns equivalent rows
 * doesn't churn the array reference.
 *
 * Sub-agents (rows with a non-null `parent_conversation_id`) are excluded:
 * they render inline inside their parent's transcript (`SubAgentActivityCard`),
 * never as independent sidebar rows. The server's list endpoints already
 * exclude them (`user_initiated = 1`), but a sub-agent snapshot can still
 * reach the store by other paths — navigating to its page, or cache
 * hydration — so the sidebar's own derivation must apply the same rule.
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
      // Sub-agents are embedded in their parent's transcript, not listed
      // as independent sidebar conversations. A non-null
      // parent_conversation_id is the structural marker of sub-agent
      // parentage (handoff / continuation conversations leave it null).
      if (c.parent_conversation_id) continue;
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
