import { useCallback, useEffect, useRef, useState } from 'react';
import { api, type PrStatusResponse } from '../api';

export type ConversationPrStatusState =
  | { status: 'disabled'; prStatus: null }
  | { status: 'loading'; prStatus: null }
  | { status: 'ready'; prStatus: PrStatusResponse };

export interface ConversationPrStatusHandle {
  state: ConversationPrStatusState;
  refresh: () => Promise<void>;
}

export function useConversationPrStatus({
  conversationId,
  convModeLabel,
  branchName,
}: {
  conversationId: string | null | undefined;
  convModeLabel: string | undefined;
  branchName: string | null | undefined;
}): ConversationPrStatusHandle {
  const [state, setState] = useState<ConversationPrStatusState>({ status: 'disabled', prStatus: null });
  const latestSeqRef = useRef(0);
  const activeScopeRef = useRef<string | null>(null);
  const scopeKey = conversationId && branchName && (convModeLabel === 'Work' || convModeLabel === 'Branch')
    ? `${conversationId}\0${branchName}\0${convModeLabel}`
    : null;

  const refresh = useCallback(async () => {
    if (!scopeKey || !conversationId) return;
    if (activeScopeRef.current !== scopeKey) return;
    const seq = ++latestSeqRef.current;
    try {
      const prStatus = await api.getPrStatus(conversationId);
      if (seq !== latestSeqRef.current || activeScopeRef.current !== scopeKey) return;
      setState({ status: 'ready', prStatus });
    } catch {
      if (seq !== latestSeqRef.current || activeScopeRef.current !== scopeKey) return;
      setState({
        status: 'ready',
        prStatus: {
          found: false,
          unavailable_reason: 'command_failed',
          refresh: {
            state: 'unavailable',
            reason: 'command_failed',
            last_attempted_at: new Date().toISOString(),
            stale: false,
          },
          work_change: { kind: 'unavailable', reason: 'command_failed' },
        },
      });
    }
  }, [conversationId, scopeKey]);

  useEffect(() => {
    latestSeqRef.current += 1;
    activeScopeRef.current = scopeKey;
    if (!scopeKey) {
      setState({ status: 'disabled', prStatus: null });
      return;
    }

    setState({ status: 'loading', prStatus: null });
    let cancelled = false;
    let timeout: number | null = null;

    const fetchStatus = async () => {
      if (cancelled) return;
      await refresh();
    };

    const schedule = () => {
      if (timeout != null) window.clearTimeout(timeout);
      timeout = window.setTimeout(async () => {
        await fetchStatus();
        if (!cancelled) schedule();
      }, 60_000);
    };

    void fetchStatus();
    schedule();

    const onVisible = () => { if (document.visibilityState === 'visible') void fetchStatus(); };
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      cancelled = true;
      latestSeqRef.current += 1;
      if (activeScopeRef.current === scopeKey) activeScopeRef.current = null;
      if (timeout != null) window.clearTimeout(timeout);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [scopeKey, refresh]);

  return {
    state,
    refresh,
  };
}
