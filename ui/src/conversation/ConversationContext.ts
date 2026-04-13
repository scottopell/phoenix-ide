import { createContext } from 'react';
import type { ConversationStore } from './ConversationStore';

/**
 * Context holds a reference to the external store, not React state. All
 * subscriptions happen per-slug via `useSyncExternalStore` inside
 * `useConversationAtom`, so context value itself never changes identity and
 * does not trigger re-renders.
 */
export const ConversationContext = createContext<ConversationStore | null>(null);
