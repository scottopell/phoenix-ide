// Codex quota snapshot store (task 67003).
//
// The codex backend emits a structured `codex.rate_limits` SSE event
// mid-stream on every turn carrying the current quota state (per-window
// usage, credits, plan). The snapshot is account-global, not per-
// conversation — any conversation's stream produces the same numbers.
//
// useConnection.ts validates the SSE event and pushes the snapshot here.
// The Settings dropdown reads via `useCodexQuota()` (useSyncExternalStore)
// so a render fires when the snapshot changes.
//
// Storage is intentionally a plain module variable, not React context:
// the data outlives any one conversation page and the consumer count is
// tiny (currently one: SettingsDropdown.CodexSection).

import { useSyncExternalStore } from 'react';
import type { QuotaDetails } from './sseSchemas';

let snapshot: QuotaDetails | null = null;
const listeners = new Set<() => void>();

function notify(): void {
  for (const fn of listeners) fn();
}

export function setCodexQuota(next: QuotaDetails): void {
  snapshot = next;
  notify();
}

/// Drop the stored snapshot. Call on codex sign-out / account switch so
/// the dropdown stops rendering stale quota for an account that no
/// longer owns this session. SSE disconnect alone does NOT invalidate
/// the snapshot — the account is unchanged across reconnects.
export function clearCodexQuota(): void {
  if (snapshot === null) return;
  snapshot = null;
  notify();
}

export function getCodexQuotaSnapshot(): QuotaDetails | null {
  return snapshot;
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function useCodexQuota(): QuotaDetails | null {
  return useSyncExternalStore(subscribe, getCodexQuotaSnapshot, getCodexQuotaSnapshot);
}
