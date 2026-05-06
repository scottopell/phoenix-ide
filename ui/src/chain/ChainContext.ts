import { createContext } from 'react';
import type { ChainStore } from './ChainStore';

/**
 * Context holds a reference to the external chain store, not React state.
 * All subscriptions happen per-rootConvId via `useSyncExternalStore` inside
 * `useChainAtom`, so context value identity never changes and consumers
 * do not re-render on store mutations they didn't subscribe to.
 */
export const ChainContext = createContext<ChainStore | null>(null);
