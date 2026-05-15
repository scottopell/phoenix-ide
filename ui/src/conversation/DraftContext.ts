import { createContext } from 'react';
import type { DraftStore } from './DraftStore';

/**
 * Sibling to `ConversationContext` that holds the per-slug draft store.
 * Keeping the draft store separate means whole-page consumers of the
 * conversation atom (e.g. `ConversationPageContent`) don't subscribe to
 * keystroke-frequency mutations at all.
 */
export const DraftContext = createContext<DraftStore | null>(null);
