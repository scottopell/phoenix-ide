import { createContext } from 'react';

type ConversationReadiness = {
  conversationId: string | null;
  confirmedLive: boolean;
};

export type ConversationReadinessContextValue = ConversationReadiness & {
  setConversationReadiness: (readiness: ConversationReadiness) => void;
};

export const defaultReadiness: ConversationReadinessContextValue = {
  conversationId: null,
  confirmedLive: false,
  setConversationReadiness: () => {},
};

export const ConversationReadinessContext = createContext<ConversationReadinessContextValue>(defaultReadiness);
