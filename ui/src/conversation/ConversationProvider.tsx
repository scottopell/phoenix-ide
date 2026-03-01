import { useCallback, useMemo, useState } from 'react';
import { conversationReducer, createInitialAtom } from './atom';
import type { SSEAction } from './atom';
import { ConversationContext } from './ConversationContext';

export function ConversationProvider({ children }: { children: React.ReactNode }) {
  const [atoms, setAtoms] = useState(
    () => new Map<string, ReturnType<typeof createInitialAtom>>()
  );

  const dispatch = useCallback((slug: string, action: SSEAction) => {
    setAtoms((prev) => {
      const current = prev.get(slug) ?? createInitialAtom();
      const next = conversationReducer(current, action);
      if (next === current) return prev; // No-op — avoid new Map allocation
      return new Map(prev).set(slug, next);
    });
  }, []);

  const value = useMemo(() => ({ atoms, dispatch }), [atoms, dispatch]);

  return (
    <ConversationContext.Provider value={value}>
      {children}
    </ConversationContext.Provider>
  );
}
