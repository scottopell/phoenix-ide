import { useCallback, useEffect, useRef, useState } from 'react';
import type { ReactionSource } from '../conversation/InlineReactionStore';

interface Options {
  scopeKey: string;
  historyKey: string;
  loading: boolean;
  hasOlder: boolean;
  error: string | null | undefined;
  locate: (source: ReactionSource) => boolean;
  loadOlder: (() => Promise<void>) | undefined;
}

interface Pending {
  source: ReactionSource;
  scopeKey: string;
  signal: AbortSignal;
  finish: (found: boolean) => void;
  requestedKey?: string;
  completed: boolean;
}

export function useReactionSourceReturn({ scopeKey, historyKey, loading, hasOlder, error, locate, loadOlder }: Options) {
  const [revision, refresh] = useState(0);
  const [pending, setPending] = useState<Pending | null>(null);
  const pendingRef = useRef<Pending | null>(null);
  const start = useCallback((source: ReactionSource, signal: AbortSignal): Promise<boolean> => {
    pendingRef.current?.finish(false);
    if (signal.aborted) return Promise.resolve(false);
    return new Promise((resolve) => {
      const abort = () => request.finish(false);
      const request: Pending = {
        source, scopeKey, signal, completed: false,
        finish(found) {
          signal.removeEventListener('abort', abort);
          if (pendingRef.current === request) {
            pendingRef.current = null;
            setPending(null);
          }
          resolve(found);
        },
      };
      signal.addEventListener('abort', abort, { once: true });
      pendingRef.current = request;
      setPending(request);
    });
  }, [scopeKey]);

  useEffect(() => {
    if (!pending || pendingRef.current !== pending) return;
    if (pending.signal.aborted || pending.scopeKey !== scopeKey) {
      pending.finish(false);
      return;
    }
    if (locate(pending.source)) {
      pending.finish(true);
      return;
    }
    if (loading) {
      return;
    }
    if (pending.requestedKey !== undefined && error) {
      // An existing error is retryable on the first explicit return request.
      if (pending.completed) pending.finish(false);
      return;
    }
    if (!hasOlder || !loadOlder) {
      pending.finish(false);
      return;
    }
    if (pending.requestedKey === historyKey) {
      if (pending.completed) pending.finish(false);
      return;
    }
    pending.requestedKey = historyKey;
    pending.completed = false;
    void loadOlder().then(() => {
      if (pendingRef.current !== pending || pending.requestedKey !== historyKey) return;
      pending.completed = true;
      refresh((value) => value + 1);
    }, () => pending.finish(false));
  }, [revision, pending, scopeKey, historyKey, loading, hasOlder, error, locate, loadOlder]);

  useEffect(() => () => { pendingRef.current?.finish(false); }, []);
  return start;
}
