/**
 * Collapsed-surface work-scope seeding hooks.
 *
 * Lives in its own module (not `WorkScopePanel.tsx`) so the component file only
 * exports components — React Fast Refresh requires that, and eslint enforces it
 * (`react-refresh/only-export-components`).
 */
import { useEffect, useState } from 'react';
import { api } from '../api';
import type { WorkScopeInventory } from '../api';
import { workScopeLiveCount } from './workScopeHelpers';

/**
 * One-shot inventory seed for a surface that may be mounted while a richer data
 * source isn't running. The collapsed file-explorer rail does not mount
 * `WorkScopeSection`, so the section's initial GET never fires; the rail's count
 * badge is then driven only by the SSE-fed `liveWorkScope`, which shows `0`
 * forever if the spawn `work_scope_update` fell outside the SSE replay window.
 *
 * This fetches the inventory once per `scopeKey` (no poll) so the badge is
 * seeded regardless of collapse. The SSE prop stays authoritative once it
 * arrives: callers merge via last-arrival-wins (see {@link useSeededLiveCount}).
 * Carries the same stale-scope guard as `useWorkScopeInventory`: a fetch for the
 * previous scope that resolves after `scopeKey` changes is rejected.
 */
function useSeededInventory(scopeKey: string | null | undefined): WorkScopeInventory | null {
  const [seeded, setSeeded] = useState<WorkScopeInventory | null>(null);

  useEffect(() => {
    if (!scopeKey) {
      setSeeded(null);
      return;
    }
    let cancelled = false;
    setSeeded(null);
    void (async () => {
      try {
        const inv = await api.getWorkScopeInventory(scopeKey);
        if (!cancelled) setSeeded(inv);
      } catch {
        // A failed seed leaves the badge on the SSE-fed value — no worse than
        // before. The expanded section retries on demand.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [scopeKey]);

  return seeded;
}

/**
 * Live-resource count for a surface that must work while collapsed, merging the
 * one-shot {@link useSeededInventory} seed with the SSE-fed `liveInventory`.
 * SSE is authoritative once present; the seed only fills the gap before the
 * first push: `liveInventory` wins when set, else the seed.
 */
export function useSeededLiveCount(
  scopeKey: string | null | undefined,
  liveInventory: WorkScopeInventory | null | undefined,
): number {
  const seeded = useSeededInventory(scopeKey);
  return workScopeLiveCount(liveInventory ?? seeded);
}
