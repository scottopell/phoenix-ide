import { useCallback, useContext, useMemo, type Dispatch } from 'react';
import { createInitialAtom } from './atom';
import type { ConversationAtom, SSEAction } from './atom';
import { ConversationContext } from './ConversationContext';
import { isAgentWorking } from '../utils';

function useConversationContext() {
  const ctx = useContext(ConversationContext);
  if (!ctx) throw new Error('useConversationAtom must be used within ConversationProvider');
  return ctx;
}

/** Returns [atom, dispatch] for the given conversation slug. */
export function useConversationAtom(slug: string): [ConversationAtom, Dispatch<SSEAction>] {
  const ctx = useConversationContext();

  const atom = useMemo(
    () => ctx.atoms.get(slug) ?? createInitialAtom(),
    [ctx.atoms, slug]
  );

  const ctxDispatch = ctx.dispatch;
  const boundDispatch = useCallback(
    (action: SSEAction) => ctxDispatch(slug, action),
    [ctxDispatch, slug]
  );

  return [atom, boundDispatch];
}

/** Derived selectors to avoid passing the raw atom to child components. */
export function useConversationSelectors(slug: string) {
  const [atom, dispatch] = useConversationAtom(slug);

  const currentTool =
    atom.phase.type === 'tool_executing' || atom.phase.type === 'cancelling_tool'
      ? atom.phase.current_tool
      : null;

  return {
    atom,
    dispatch,
    isAgentWorking: isAgentWorking(atom.phase),
    currentTool,
    streamingText: atom.streamingBuffer?.text ?? null,
    breadcrumbs: atom.breadcrumbs,
    isOffline:
      atom.connectionState === 'reconnecting' || atom.connectionState === 'failed',
    isLive: atom.connectionState === 'live',
  };
}
