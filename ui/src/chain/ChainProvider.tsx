import { useRef } from 'react';
import { ChainStore } from './ChainStore';
import { ChainContext } from './ChainContext';

export function ChainProvider({ children }: { children: React.ReactNode }) {
  // Single store instance for the app. Refs are fine here because the
  // store is mutated externally and subscriptions run through
  // `useSyncExternalStore`.
  const storeRef = useRef<ChainStore | null>(null);
  if (storeRef.current === null) {
    storeRef.current = new ChainStore();
  }

  return (
    <ChainContext.Provider value={storeRef.current}>
      {children}
    </ChainContext.Provider>
  );
}
