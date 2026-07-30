import { useSyncExternalStore } from 'react';
import type { QuotaDetails } from './sseSchemas';

let snapshot: QuotaDetails | null = null;
const listeners = new Set<() => void>();

function notify(): void {
  for (const listener of listeners) listener();
}

export function setCodexQuota(next: QuotaDetails): void {
  snapshot = next;
  notify();
}

export function clearCodexQuota(): void {
  if (snapshot === null) return;
  snapshot = null;
  notify();
}

export function getCodexQuotaSnapshot(): QuotaDetails | null {
  return snapshot;
}

export function useCodexQuota(): QuotaDetails | null {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    getCodexQuotaSnapshot,
    getCodexQuotaSnapshot,
  );
}
