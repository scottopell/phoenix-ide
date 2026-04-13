import { useCallback, useContext, useSyncExternalStore, type Dispatch } from 'react';
import type { ConversationAtom, SSEAction } from './atom';
import { ConversationContext } from './ConversationContext';
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
    isOffline:
      atom.connectionState === 'reconnecting' || atom.connectionState === 'failed',
    isLive: atom.connectionState === 'live',
  };
}
