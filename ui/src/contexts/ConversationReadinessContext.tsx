import { type ReactNode, useCallback, useMemo, useState } from 'react';
import { ConversationReadinessContext } from './conversationReadinessCore';
import type { ConversationReadinessContextValue } from './conversationReadinessCore';

export function ConversationReadinessProvider({ children }: { children: ReactNode }) {
  const [readiness, setReadiness] = useState<Pick<ConversationReadinessContextValue, 'conversationId' | 'confirmedLive'>>({
    conversationId: null,
    confirmedLive: false,
  });

  const setConversationReadiness = useCallback((next: Pick<ConversationReadinessContextValue, 'conversationId' | 'confirmedLive'>) => {
    setReadiness(next);
  }, []);

  const value = useMemo(() => ({
    ...readiness,
    setConversationReadiness,
  }), [readiness, setConversationReadiness]);

  return (
    <ConversationReadinessContext.Provider value={value}>
      {children}
    </ConversationReadinessContext.Provider>
  );
}
