import { api, type ModelsResponse } from './api';

/**
 * Shared models/credential poller.
 *
 * Centralizes what used to be three independent polling loops in
 * ConversationPage and ConversationListPage, each calling `api.listModels()`
 * every 5s. A single loop now runs for as long as at least one component
 * has subscribed, and the interval adapts based on credential health.
 *
 * Guarantees:
 *   - Concurrent callers share one in-flight request (Promise dedup).
 *   - Repeat reads within `TTL_MS` return the cached response.
 *   - The poller starts on the first subscribe and stops when the last
 *     subscriber unsubscribes.
 *   - Tab-hidden: the next fetch is skipped but the loop keeps ticking so we
 *     resume immediately when the tab becomes visible.
 */

type Listener = (resp: ModelsResponse) => void;

const HEALTHY_INTERVAL_MS = 30_000;
const UNHEALTHY_INTERVAL_MS = 5_000;
const TTL_MS = 2_000;

let cached: { data: ModelsResponse; at: number } | null = null;
let inFlightPromise: Promise<ModelsResponse> | null = null;
let pollTimer: ReturnType<typeof setTimeout> | null = null;
const listeners = new Set<Listener>();

function emit(data: ModelsResponse) {
  for (const listener of listeners) {
    listener(data);
  }
}

function fetchNow(): Promise<ModelsResponse> {
  if (inFlightPromise !== null) return inFlightPromise;
  inFlightPromise = api.listModels().then(
    (data) => {
      cached = { data, at: Date.now() };
      inFlightPromise = null;
      emit(data);
      return data;
    },
    (err) => {
      inFlightPromise = null;
      throw err;
    },
  );
  return inFlightPromise;
}

function nextIntervalMs(): number {
  const status = cached?.data.credential_status;
  // 'valid' → healthy long interval; everything else (required, failed,
  // running, not_configured) → short interval so the auth panel reacts
  // promptly when the credential needs attention.
  return status === 'valid' ? HEALTHY_INTERVAL_MS : UNHEALTHY_INTERVAL_MS;
}

function schedulePoll() {
  if (pollTimer !== null) clearTimeout(pollTimer);
  pollTimer = setTimeout(async () => {
    if (listeners.size === 0) {
      pollTimer = null;
      return;
    }
    if (document.visibilityState === 'visible') {
      try {
        await fetchNow();
      } catch {
        // silent — next tick will retry
      }
    }
    schedulePoll();
  }, nextIntervalMs());
}

/**
 * Get the latest models response, using the shared cache when fresh.
 * Use this for one-shot reads (e.g. after a user action) rather than
 * subscribing.
 */
export async function getModels(options: { force?: boolean } = {}): Promise<ModelsResponse> {
  if (!options.force && cached && Date.now() - cached.at < TTL_MS) {
    return cached.data;
  }
  return fetchNow();
}

/**
 * Subscribe to model/credential updates. The shared polling loop starts on
 * the first subscriber and stops when the last one unsubscribes.
 */
export function subscribeModels(listener: Listener): () => void {
  listeners.add(listener);
  if (cached) {
    // Emit cached value immediately so the new subscriber doesn't wait for the
    // next poll tick. Do not emit inline — defer to the next microtask so the
    // caller's setState is not invoked during render.
    queueMicrotask(() => {
      if (cached && listeners.has(listener)) listener(cached.data);
    });
  } else {
    void fetchNow().catch(() => {
      /* silent — polling loop will retry */
    });
  }
  if (pollTimer === null) {
    schedulePoll();
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && pollTimer !== null) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
  };
}

/**
 * Force an immediate refresh and broadcast to subscribers. Use after a user
 * action that may have changed credential state (e.g. the auth panel closes).
 */
export async function refreshModels(): Promise<ModelsResponse> {
  return fetchNow();
}
