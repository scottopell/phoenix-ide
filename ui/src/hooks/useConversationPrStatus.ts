import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  api,
  type ActivePrSelectionResponse,
  type AssociatedPrStatusEnvelope,
  type AssociatedPrSummaryResponse,
  type CachedPrSummary,
  type PinAssociatedPrRequest,
  type PrStatusResponse,
} from '../api';

export type ConversationPrStatusState =
  | { status: 'disabled'; prStatus: null }
  | { status: 'loading'; prStatus: null }
  | { status: 'ready'; prStatus: PrStatusResponse };

type InternalConversationPrStatusState = ConversationPrStatusState & { scopeKey: string | null };

export interface ConversationPrStatusHandle {
  state: ConversationPrStatusState;
  refresh: () => Promise<PrStatusResponse | undefined>;
  refreshForSafety: () => Promise<PrStatusResponse | undefined>;
  refreshAfterMutation: () => Promise<PrStatusResponse | undefined>;
  activeSelection?: AssociatedPrStatusEnvelope | null;
  activePrSummary?: AssociatedPrSummaryResponse | null;
  ambiguous?: boolean;
  pinActivePr?: (request: PinAssociatedPrRequest) => Promise<void>;
  resumeInference?: () => Promise<void>;
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

function isActionablePr(pr: AssociatedPrSummaryResponse): boolean {
  return pr.display_state === 'open' || pr.display_state === 'draft';
}

function samePrIdentity(
  activePr: ActivePrSelectionResponse | undefined,
  pr: AssociatedPrSummaryResponse,
): boolean {
  return activePr?.pr.repo_owner === pr.repo_owner
    && activePr?.pr.repo_name === pr.repo_name
    && activePr?.pr.pr_number === pr.pr_number;
}

function selectionFromPrStatus(prStatus: PrStatusResponse): AssociatedPrStatusEnvelope | null {
  if (prStatus.selection) return prStatus.selection;
  if (prStatus.associated_prs) {
    return {
      associated_prs: prStatus.associated_prs,
      ...(prStatus.active_pr ? { active_pr: prStatus.active_pr } : {}),
      ...(prStatus.latest_observed_branch
        ? { latest_observed_branch: prStatus.latest_observed_branch }
        : {}),
    };
  }
  return null;
}

function activePrSummaryFromSelection(selection: AssociatedPrStatusEnvelope | null): AssociatedPrSummaryResponse | null {
  if (!selection) return null;
  return selection.associated_prs.find((pr) => samePrIdentity(selection.active_pr, pr)) ?? null;
}

function isSelectionAmbiguous(selection: AssociatedPrStatusEnvelope | null): boolean {
  if (!selection || selection.active_pr) return false;
  return selection.associated_prs.filter(isActionablePr).length > 1;
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
    if (shouldShowCachedSeed(internalState, cachedSeed) && cachedSeed) {
      const liveSelection = internalState.status === 'ready'
        ? selectionFromPrStatus(internalState.prStatus)
        : null;
      return {
        status: 'ready',
        prStatus: liveSelection
          ? {
              ...cachedSeed,
              associated_prs: liveSelection.associated_prs,
              ...(liveSelection.active_pr ? { active_pr: liveSelection.active_pr } : {}),
              ...(liveSelection.latest_observed_branch
                ? { latest_observed_branch: liveSelection.latest_observed_branch }
                : {}),
            }
          : cachedSeed,
      };
    }
    return internalState;
  }
  if (cachedSeed) return { status: 'ready', prStatus: cachedSeed };
  return { status: 'loading', prStatus: null };
}

const ROUTINE_REFRESH_FRESHNESS_MS = 10_000;
const IN_FLIGHT_REUSE_MS = 30_000;

type PrStatusRefreshIntent = 'background' | 'explicit' | 'safety' | 'post-mutation';

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
  const activationGenerationRef = useRef(0);
  const activeScopeRef = useRef<string | null>(null);
  const inFlightRef = useRef<{
    scopeKey: string;
    seq: number;
    intent: PrStatusRefreshIntent;
    monotonicStartedAt: number;
    wallStartedAt: number;
    promise: Promise<PrStatusResponse | undefined>;
  } | null>(null);
  const lastCompletedRef = useRef<{
    scopeKey: string;
    monotonicAt: number;
    wallAt: number;
  } | null>(null);
  const scopeKey = conversationId && branchName && (convModeLabel === 'Work' || convModeLabel === 'Branch')
    ? `${conversationId}\0${branchName}\0${convModeLabel}`
    : null;
  const cachedSeed = useMemo(
    () => (scopeKey && cachedPr ? cachedPrToStatus(cachedPr) : null),
    [cachedPr, scopeKey],
  );
  const cachedSelection: AssociatedPrStatusEnvelope | null = null;
  const cachedSeedRef = useRef<PrStatusResponse | null>(null);
  cachedSeedRef.current = cachedSeed;
  const [internalState, setInternalState] = useState<InternalConversationPrStatusState>(() => (
    cachedSeed && scopeKey
      ? { scopeKey, status: 'ready', prStatus: cachedSeed }
      : { scopeKey: null, status: 'disabled', prStatus: null }
  ));

  const startRefresh = useCallback((intent: PrStatusRefreshIntent): Promise<PrStatusResponse | undefined> => {
    if (!scopeKey || !conversationId || activeScopeRef.current !== scopeKey) {
      return Promise.resolve(undefined);
    }
    const current = inFlightRef.current;
    const currentMonotonicAge = current ? performance.now() - current.monotonicStartedAt : -1;
    const currentWallAge = current ? Date.now() - current.wallStartedAt : -1;
    const currentIsLive = current?.scopeKey === scopeKey
      && current.seq === latestSeqRef.current
      && currentMonotonicAge >= 0
      && currentWallAge >= 0
      && currentMonotonicAge < IN_FLIGHT_REUSE_MS
      && currentWallAge < IN_FLIGHT_REUSE_MS;
    const currentIsReusable = currentIsLive && intent === 'background';
    if (currentIsReusable) return current.promise;

    const seq = ++latestSeqRef.current;
    const monotonicStartedAt = performance.now();
    const wallStartedAt = Date.now();
    inFlightRef.current = null;
    const promise = (async () => {
      try {
        const prStatus = await api.getPrStatus(conversationId);
        if (seq !== latestSeqRef.current || activeScopeRef.current !== scopeKey) return undefined;
        setInternalState({ scopeKey, status: 'ready', prStatus });
        lastCompletedRef.current = prStatus.refresh.state !== 'unavailable' && !prStatus.refresh.stale
          ? { scopeKey, monotonicAt: performance.now(), wallAt: Date.now() }
          : null;
        return prStatus;
      } catch {
        if (seq !== latestSeqRef.current || activeScopeRef.current !== scopeKey) return undefined;
        lastCompletedRef.current = null;
        const fallback = cachedSeedRef.current;
        if (fallback) {
          const unavailable: PrStatusResponse = {
            ...fallback,
            unavailable_reason: 'command_failed',
            refresh: {
              ...fallback.refresh,
              state: 'unavailable',
              reason: 'command_failed',
              last_attempted_at: new Date().toISOString(),
              stale: true,
            },
          };
          setInternalState({ scopeKey, status: 'ready', prStatus: unavailable });
          return unavailable;
        }
        const unavailable: PrStatusResponse = {
          found: false,
          unavailable_reason: 'command_failed',
          refresh: {
            state: 'unavailable',
            reason: 'command_failed',
            last_attempted_at: new Date().toISOString(),
            stale: false,
          },
          work_change: { kind: 'unavailable', reason: 'command_failed' },
        };
        setInternalState({ scopeKey, status: 'ready', prStatus: unavailable });
        return unavailable;
      } finally {
        if (inFlightRef.current?.scopeKey === scopeKey && inFlightRef.current.seq === seq) {
          inFlightRef.current = null;
        }
      }
    })();
    inFlightRef.current = {
      scopeKey,
      seq,
      intent,
      monotonicStartedAt,
      wallStartedAt,
      promise,
    };
    return promise;
  }, [conversationId, scopeKey]);

  const refresh = useCallback(
    () => startRefresh('explicit'),
    [startRefresh],
  );

  const refreshForSafety = useCallback(async () => {
    const activationGeneration = activationGenerationRef.current;
    let result = await startRefresh('safety');
    while (!result && scopeKey && activeScopeRef.current === scopeKey
      && activationGenerationRef.current === activationGeneration) {
      const replacement = inFlightRef.current?.scopeKey === scopeKey
        ? inFlightRef.current.promise
        : null;
      result = replacement ? await replacement : await startRefresh('safety');
    }
    return activationGenerationRef.current === activationGeneration ? result : undefined;
  }, [scopeKey, startRefresh]);

  const refreshRoutine = useCallback((): Promise<PrStatusResponse | undefined> => {
    if (!scopeKey || activeScopeRef.current !== scopeKey) return Promise.resolve(undefined);
    const lastCompleted = lastCompletedRef.current;
    if (lastCompleted?.scopeKey === scopeKey) {
      const monotonicElapsed = performance.now() - lastCompleted.monotonicAt;
      const wallElapsed = Date.now() - lastCompleted.wallAt;
      if (monotonicElapsed >= 0 && wallElapsed >= 0
        && monotonicElapsed < ROUTINE_REFRESH_FRESHNESS_MS
        && wallElapsed < ROUTINE_REFRESH_FRESHNESS_MS) {
        return Promise.resolve(undefined);
      }
    }
    return startRefresh('background');
  }, [scopeKey, startRefresh]);

  useEffect(() => {
    activationGenerationRef.current += 1;
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
      await refreshRoutine();
    };

    const schedule = () => {
      if (timeout != null) window.clearTimeout(timeout);
      timeout = window.setTimeout(() => {
        void fetchStatus();
        if (!cancelled) schedule();
      }, 60_000);
    };

    void startRefresh('background');
    schedule();

    const onVisible = () => { if (document.visibilityState === 'visible') void fetchStatus(); };
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      cancelled = true;
      activationGenerationRef.current += 1;
      latestSeqRef.current += 1;
      if (inFlightRef.current?.scopeKey === scopeKey) inFlightRef.current = null;
      if (activeScopeRef.current === scopeKey) activeScopeRef.current = null;
      if (timeout != null) window.clearTimeout(timeout);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [scopeKey, refreshRoutine, startRefresh]);

  const publicState = publicStateForScope(internalState, scopeKey, cachedSeed);
  const liveSelection = internalState.scopeKey === scopeKey && internalState.status === 'ready'
    ? selectionFromPrStatus(internalState.prStatus)
    : null;
  const activeSelection = liveSelection ?? (publicState.status === 'ready'
    ? (selectionFromPrStatus(publicState.prStatus) ?? cachedSelection)
    : cachedSelection);

  const refreshAfterMutation = useCallback(
    () => startRefresh('post-mutation'),
    [startRefresh],
  );

  const pinActivePr = useCallback(async (request: PinAssociatedPrRequest) => {
    if (!scopeKey || !conversationId) return;
    await api.pinAssociatedPr(conversationId, request);
    await refreshAfterMutation();
  }, [conversationId, refreshAfterMutation, scopeKey]);

  const resumeInference = useCallback(async () => {
    if (!scopeKey || !conversationId) return;
    await api.resumeAssociatedPrInference(conversationId);
    await refreshAfterMutation();
  }, [conversationId, refreshAfterMutation, scopeKey]);

  return {
    state: publicState,
    refresh,
    refreshForSafety,
    refreshAfterMutation,
    activeSelection,
    activePrSummary: activePrSummaryFromSelection(activeSelection),
    ambiguous: isSelectionAmbiguous(activeSelection),
    pinActivePr,
    resumeInference,
  };
}
