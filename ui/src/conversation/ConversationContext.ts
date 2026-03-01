import { createContext } from 'react';
import type { ConversationAtom, SSEAction } from './atom';

export interface ConversationContextValue {
  atoms: Map<string, ConversationAtom>;
  dispatch: (slug: string, action: SSEAction) => void;
}

export const ConversationContext = createContext<ConversationContextValue | null>(null);
