import { useCallback, useContext } from 'react';
import { ConversationContext } from './ConversationContext';
import { api, type Conversation } from '../api';

/**
 * Wraps `api.createConversation` to write the returned `Conversation`
 * into the store before returning. Without this, the sidebar (which
 * derives rows from `store.listSnapshots()`) lies for up to one network
 * round-trip + 5s poll tick after the user creates a conversation.
 *
 * Navigation is left to the caller so per-site ordering (e.g. the
 * `seed-draft:<id>` localStorage write in TaskViewer) stays intact.
 */
export function useCreateConversationWithStore() {
  const store = useContext(ConversationContext);
  if (!store)
    throw new Error(
      'useCreateConversationWithStore must be used within ConversationProvider',
    );
  return useCallback(
    async (
      ...args: Parameters<typeof api.createConversation>
    ): Promise<Conversation> => {
      const conv = await api.createConversation(...args);
      store.upsertSnapshot(conv.slug, conv);
      return conv;
    },
    [store],
  );
}
