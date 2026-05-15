import { createContext } from 'react';
import type { DraftStore } from './DraftStore';

/** Holds the per-slug `DraftStore`. See `ConversationProvider` for why
 *  the draft store is separate from `ConversationStore`. */
export const DraftContext = createContext<DraftStore | null>(null);
