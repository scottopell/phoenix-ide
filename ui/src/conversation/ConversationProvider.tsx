import { useRef } from 'react';
import { ConversationStore } from './ConversationStore';
import { ConversationContext } from './ConversationContext';

export function ConversationProvider({ children }: { children: React.ReactNode }) {
  // Single store instance for the app. Refs are fine here because the store is
  // mutable externally and subscriptions run through `useSyncExternalStore`.
  const storeRef = useRef<ConversationStore | null>(null);
  if (storeRef.current === null) {
    storeRef.current = new ConversationStore();
  }

  return (
    <ConversationContext.Provider value={storeRef.current}>
      {children}
    </ConversationContext.Provider>
  );
}
