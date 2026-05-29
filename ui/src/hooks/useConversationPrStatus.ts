import { useEffect, useState } from 'react';
import { api, type PrStatusResponse } from '../api';

export type ConversationPrStatusState =
  | { status: 'disabled'; prStatus: null }
  | { status: 'loading'; prStatus: null }
  | { status: 'ready'; prStatus: PrStatusResponse };

export interface ConversationPrStatusHandle {
  state: ConversationPrStatusState;
  manualFallbackEnabled: boolean;
  enableManualFallback: () => void;
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

  useEffect(() => {
    setManualFallbackEnabled(false);
    if (!conversationId || !branchName || (convModeLabel !== 'Work' && convModeLabel !== 'Branch')) {
      setState({ status: 'disabled', prStatus: null });
      return;
    }

    setState({ status: 'loading', prStatus: null });
    let cancelled = false;
    let timeout: number | null = null;
    let latestSeq = 0;

    const fetchStatus = async () => {
      const seq = ++latestSeq;
      const fresh = () => !cancelled && seq === latestSeq;
      try {
        const prStatus = await api.getPrStatus(conversationId);
        if (!fresh()) return;
        setState({ status: 'ready', prStatus });
        if (!prStatus.unavailable_reason) setManualFallbackEnabled(false);
      } catch {
        if (fresh()) setState({ status: 'ready', prStatus: { found: false, unavailable_reason: 'command_failed' } });
      }
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
      if (timeout != null) window.clearTimeout(timeout);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [conversationId, convModeLabel, branchName]);

  return {
    state,
    manualFallbackEnabled,
    enableManualFallback: () => setManualFallbackEnabled(true),
  };
}
