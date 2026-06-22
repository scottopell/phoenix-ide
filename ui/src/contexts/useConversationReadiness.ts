import { useContext } from 'react';
import { ConversationReadinessContext } from './conversationReadinessCore';

export function useConversationReadiness() {
  return useContext(ConversationReadinessContext);
}
