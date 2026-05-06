import { useRef } from 'react';
import { ConversationStore } from './ConversationStore';
import { ConversationContext } from './ConversationContext';
import { useConversationsRefresh } from './useConversationsRefresh';

/**
 * Mounts the conversation store and the periodic refresh service that
 * keeps it in sync with the server. Every consumer of conversation data
 * — sidebar, list page, conversation page — reads through this single
 * provider; per-component polling and parallel `Conversation[]` state
 * are gone (task 08684).
 */
export function ConversationProvider({ children }: { children: React.ReactNode }) {
  // Single store instance for the app. Refs are fine here because the store is
  // mutable externally and subscriptions run through `useSyncExternalStore`.
  const storeRef = useRef<ConversationStore | null>(null);
  if (storeRef.current === null) {
    storeRef.current = new ConversationStore();
  }

  return (
    <ConversationContext.Provider value={storeRef.current}>
      <ConversationsRefreshDriver />
      {children}
    </ConversationContext.Provider>
  );
}

/**
 * Internal: lives inside the provider so it can read the context the
 * provider just installed. The hook handles the polling + cache + online
 * + hard-delete listeners; this component is just a scope.
 */
function ConversationsRefreshDriver() {
  useConversationsRefresh();
  return null;
}
