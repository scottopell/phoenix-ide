import { useCallback, useEffect, useRef, useState } from 'react';
import { api, type PrStatusResponse } from '../api';

export type ConversationPrStatusState =
  | { status: 'disabled'; prStatus: null }
  | { status: 'loading'; prStatus: null }
  | { status: 'ready'; prStatus: PrStatusResponse };

export interface ConversationPrStatusHandle {
  state: ConversationPrStatusState;
  manualFallbackEnabled: boolean;
  enableManualFallback: () => void;
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
  const [manualFallbackEnabled, setManualFallbackEnabled] = useState(false);
  const latestSeqRef = useRef(0);

  const refresh = useCallback(async () => {
    if (!conversationId || !branchName || (convModeLabel !== 'Work' && convModeLabel !== 'Branch')) {
      setState({ status: 'disabled', prStatus: null });
      return;
    }
    const seq = ++latestSeqRef.current;
    try {
      const prStatus = await api.getPrStatus(conversationId);
      if (seq !== latestSeqRef.current) return;
      setState({ status: 'ready', prStatus });
      if (!prStatus.unavailable_reason) setManualFallbackEnabled(false);
    } catch {
      if (seq !== latestSeqRef.current) return;
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
        },
      });
    }
  }, [conversationId, convModeLabel, branchName]);

  useEffect(() => {
    setManualFallbackEnabled(false);
    latestSeqRef.current += 1;
    if (!conversationId || !branchName || (convModeLabel !== 'Work' && convModeLabel !== 'Branch')) {
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
      if (timeout != null) window.clearTimeout(timeout);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [conversationId, convModeLabel, branchName, refresh]);

  return {
    state,
    manualFallbackEnabled,
    enableManualFallback: () => setManualFallbackEnabled(true),
    refresh,
  };
}
