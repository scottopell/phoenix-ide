import { useCallback, useEffect, useRef } from 'react';

/** DB-backed refresh shared by the reminder panel and connected-page alerts. */
export function useReminderRefresh<T>(
  load: (signal: AbortSignal) => Promise<T>,
  receive: (value: T) => void,
  fail: (error: unknown) => void,
): () => void {
  const refreshRef = useRef<() => void>(() => {});
  useEffect(() => {
    const controller = new AbortController();
    let running = false;
    let requested = false;
    const refresh = async () => {
      if (running) { requested = true; return; }
      running = true;
      do {
        requested = false;
        try {
          const value = await load(controller.signal);
          if (!controller.signal.aborted) receive(value);
        } catch (error) {
          if (!controller.signal.aborted) fail(error);
        }
      } while (requested && !controller.signal.aborted);
      running = false;
    };
    const trigger = () => { void refresh(); };
    refreshRef.current = trigger;
    trigger();
    const timer = window.setInterval(trigger, 5000);
    const visible = () => { if (document.visibilityState === 'visible') trigger(); };
    window.addEventListener('online', trigger);
    window.addEventListener('focus', trigger);
    document.addEventListener('visibilitychange', visible);
    return () => {
      controller.abort();
      window.clearInterval(timer);
      window.removeEventListener('online', trigger);
      window.removeEventListener('focus', trigger);
      document.removeEventListener('visibilitychange', visible);
      refreshRef.current = () => {};
    };
  }, [load, receive, fail]);
  return useCallback(() => refreshRef.current(), []);
}
