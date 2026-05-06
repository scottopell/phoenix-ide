import { useCallback, useContext, useSyncExternalStore, type Dispatch } from 'react';
import type { ChainAtom, ChainAction } from './chainAtom';
import { ChainContext } from './ChainContext';

function useChainStore() {
  const store = useContext(ChainContext);
  if (!store) throw new Error('useChainAtom must be used within ChainProvider');
  return store;
}

/**
 * Returns [atom, dispatch] for the given chain rootConvId.
 *
 * Subscribes only to this rootConvId's atom via the external store —
 * updates to other chains do not cause this hook to re-render.
 *
 * Pass `null` (e.g. before route params load) to opt out of subscription;
 * dispatch is still returned but is a no-op routed to a sentinel key.
 */
export function useChainAtom(
  rootConvId: string | null,
): [ChainAtom, Dispatch<ChainAction>] {
  const store = useChainStore();
  // Sentinel key for the null case. The atom returned for the sentinel
  // is created on first access and never observed by anyone else, so it
  // is benignly leaked. Dispatch into it is a no-op as far as the UI
  // can observe.
  const key = rootConvId ?? '__null__';

  const subscribe = useCallback(
    (listener: () => void) => store.subscribe(key, listener),
    [store, key],
  );
  const getSnapshot = useCallback(() => store.getSnapshot(key), [store, key]);

  const atom = useSyncExternalStore(subscribe, getSnapshot);

  const dispatch = useCallback(
    (action: ChainAction) => store.dispatch(key, action),
    [store, key],
  );

  return [atom, dispatch];
}
