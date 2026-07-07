import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, type CachedPrSummary, type PrStatusResponse } from '../api';

export type ConversationPrStatusState =
  | { status: 'disabled'; prStatus: null }
  | { status: 'loading'; prStatus: null }
  | { status: 'ready'; prStatus: PrStatusResponse };

type InternalConversationPrStatusState = ConversationPrStatusState & { scopeKey: string | null };

export interface ConversationPrStatusHandle {
  state: ConversationPrStatusState;
  refresh: () => Promise<void>;
}

function displayStateToGhState(displayState: CachedPrSummary['display_state']): string {
  return displayState === 'open' || displayState === 'draft' ? 'OPEN' : 'CLOSED';
}

function cachedPrToStatus(cachedPr: CachedPrSummary, attemptedAt = new Date().toISOString()): PrStatusResponse {
  const draft = cachedPr.display_state === 'draft';
  const state = displayStateToGhState(cachedPr.display_state);
  const feedbackStatus = cachedPr.feedback_status ?? 'open';
  return {
    found: true,
    number: cachedPr.number,
    title: cachedPr.title,
    url: cachedPr.url,
    state,
    draft,
    base: cachedPr.base,
    head: cachedPr.head,
    display_state: cachedPr.display_state,
    feedback_status: feedbackStatus,
    pr: {
      number: cachedPr.number,
      title: cachedPr.title,
      url: cachedPr.url,
      state,
      draft,
      display_state: cachedPr.display_state,
      base: cachedPr.base,
      head: cachedPr.head,
    },
    refresh: {
      state: 'unavailable',
      reason: 'command_failed',
      last_attempted_at: attemptedAt,
      stale: true,
    },
    work_change: { kind: 'loading' },
  };
}

function cachedSeedMatchesStatus(cachedSeed: PrStatusResponse, prStatus: PrStatusResponse): boolean {
  return prStatus.found === true
    && (prStatus.number ?? prStatus.pr?.number) === cachedSeed.number
    && (prStatus.title ?? prStatus.pr?.title) === cachedSeed.title
    && (prStatus.url ?? prStatus.pr?.url) === cachedSeed.url
    && prStatus.display_state === cachedSeed.display_state
    && (prStatus.feedback_status ?? 'open') === (cachedSeed.feedback_status ?? 'open')
    && (prStatus.base ?? prStatus.pr?.base) === cachedSeed.base
    && (prStatus.head ?? prStatus.pr?.head) === cachedSeed.head;
}

function shouldShowCachedSeed(
  internalState: InternalConversationPrStatusState,
  cachedSeed: PrStatusResponse | null,
): cachedSeed is PrStatusResponse {
  if (!cachedSeed) return false;
  if (internalState.status !== 'ready') return true;
  if (internalState.prStatus.found && internalState.prStatus.refresh.state === 'fresh' && !internalState.prStatus.refresh.stale) {
    return false;
  }
  return !cachedSeedMatchesStatus(cachedSeed, internalState.prStatus);
}

function publicStateForScope(
  internalState: InternalConversationPrStatusState,
  scopeKey: string | null,
  cachedSeed: PrStatusResponse | null,
): ConversationPrStatusState {
  if (!scopeKey) return { status: 'disabled', prStatus: null };
  if (internalState.scopeKey === scopeKey) {
    if (shouldShowCachedSeed(internalState, cachedSeed)) return { status: 'ready', prStatus: cachedSeed };
    return internalState;
  }
  if (cachedSeed) return { status: 'ready', prStatus: cachedSeed };
  return { status: 'loading', prStatus: null };
}

export function useConversationPrStatus({
  conversationId,
  convModeLabel,
  branchName,
  cachedPr,
}: {
  conversationId: string | null | undefined;
  convModeLabel: string | undefined;
  branchName: string | null | undefined;
  cachedPr?: CachedPrSummary | null | undefined;
}): ConversationPrStatusHandle {
  const latestSeqRef = useRef(0);
  const activeScopeRef = useRef<string | null>(null);
  const scopeKey = conversationId && branchName && (convModeLabel === 'Work' || convModeLabel === 'Branch')
    ? `${conversationId}\0${branchName}\0${convModeLabel}`
    : null;
  const cachedSeed = useMemo(
    () => (scopeKey && cachedPr ? cachedPrToStatus(cachedPr) : null),
    [cachedPr, scopeKey],
  );
  const cachedSeedRef = useRef<PrStatusResponse | null>(null);
  cachedSeedRef.current = cachedSeed;
  const [internalState, setInternalState] = useState<InternalConversationPrStatusState>(() => (
    cachedSeed && scopeKey
      ? { scopeKey, status: 'ready', prStatus: cachedSeed }
      : { scopeKey: null, status: 'disabled', prStatus: null }
  ));

  const refresh = useCallback(async () => {
    if (!scopeKey || !conversationId) return;
    if (activeScopeRef.current !== scopeKey) return;
    const seq = ++latestSeqRef.current;
    try {
      const prStatus = await api.getPrStatus(conversationId);
      if (seq !== latestSeqRef.current || activeScopeRef.current !== scopeKey) return;
      setInternalState({ scopeKey, status: 'ready', prStatus });
    } catch {
      if (seq !== latestSeqRef.current || activeScopeRef.current !== scopeKey) return;
      const fallback = cachedSeedRef.current;
      if (fallback) {
        setInternalState({
          scopeKey,
          status: 'ready',
          prStatus: {
            ...fallback,
            unavailable_reason: 'command_failed',
            refresh: {
              ...fallback.refresh,
              state: 'unavailable',
              reason: 'command_failed',
              last_attempted_at: new Date().toISOString(),
              stale: true,
            },
          },
        });
        return;
      }
      setInternalState({
        scopeKey,
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
      setInternalState({ scopeKey: null, status: 'disabled', prStatus: null });
      return;
    }

    const seedForScope = cachedSeedRef.current;
    setInternalState(seedForScope
      ? { scopeKey, status: 'ready', prStatus: seedForScope }
      : { scopeKey, status: 'loading', prStatus: null });
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
    state: publicStateForScope(internalState, scopeKey, cachedSeed),
    refresh,
  };
}
