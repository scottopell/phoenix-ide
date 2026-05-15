import { useRef } from 'react';
import { ConversationStore } from './ConversationStore';
import { ConversationContext } from './ConversationContext';
import { DraftStore } from './DraftStore';
import { DraftContext } from './DraftContext';
import { useConversationsRefreshDriver } from './useConversationsRefresh';

/**
 * Mounts the conversation store and the periodic refresh service that
 * keeps it in sync with the server. Every consumer of conversation data
 * — sidebar, list page, conversation page — reads through this single
 * provider; per-component polling and parallel `Conversation[]` state
 * are gone (task 08684).
 *
 * Also mounts a sibling per-slug `DraftStore`. Drafts live in a separate
 * store so keystroke mutations don't invalidate the conversation atom's
 * whole-snapshot subscriptions (Codex review on PR #92).
 */
export function ConversationProvider({ children }: { children: React.ReactNode }) {
  // Single store instances for the app. Refs are fine here because the stores
  // are mutable externally and subscriptions run through `useSyncExternalStore`.
  const storeRef = useRef<ConversationStore | null>(null);
  if (storeRef.current === null) {
    storeRef.current = new ConversationStore();
  }
  const draftStoreRef = useRef<DraftStore | null>(null);
  if (draftStoreRef.current === null) {
    draftStoreRef.current = new DraftStore();
  }

  return (
    <ConversationContext.Provider value={storeRef.current}>
      <DraftContext.Provider value={draftStoreRef.current}>
        <ConversationsRefreshDriver />
        {children}
      </DraftContext.Provider>
    </ConversationContext.Provider>
  );
}

/**
 * Internal: lives inside the provider so it can read the context the
 * provider just installed. The driver hook installs the polling +
 * online + hard-delete listeners; this component is the single mount
 * point for those side effects. Other consumers grab `refresh` via
 * `useConversationsRefresh`, which is now side-effect-free.
 */
function ConversationsRefreshDriver() {
  useConversationsRefreshDriver();
  return null;
}
