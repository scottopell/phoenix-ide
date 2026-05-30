/**
 * Dev-only render profiler. Wraps a subtree in React's <Profiler> and
 * aggregates per-region commit counts + durations so we can SEE what
 * actually re-renders (and how expensively) during a real streaming
 * session, instead of guessing from static reads.
 *
 * Zero-overhead unless explicitly enabled: when disabled (or in a
 * production build) <RenderProfiler> renders its children directly with no
 * <Profiler> wrapper and installs nothing on `window`.
 *
 * Enable in the browser console, then reload:
 *
 *     __phoenixProfiler.enable()      // sets a localStorage flag + reloads
 *     // ...drive a streaming turn...
 *     __phoenixProfiler.dump()        // console.table of per-region stats
 *     __phoenixProfiler.auto(2000)    // or: auto-dump every 2s
 *     __phoenixProfiler.reset()       // zero the counters
 *     __phoenixProfiler.disable()     // clear the flag + reload
 *
 * Or load any page with `?profile=1`.
 */
import { Profiler, type ProfilerOnRenderCallback, type ReactNode } from 'react';

const FLAG_KEY = 'phoenix:profile';

function readEnabled(): boolean {
  if (!import.meta.env.DEV) return false;
  try {
    if (localStorage.getItem(FLAG_KEY) === '1') return true;
  } catch {
    // localStorage unavailable — fall through to the query-param check
  }
  return typeof location !== 'undefined' && /[?&]profile=1\b/.test(location.search);
}

// Computed once at module load. Toggling goes through enable()/disable(),
// which persist the flag and reload — a render-profiler that flips mid-session
// would itself perturb the measurements it exists to take.
const ENABLED = readEnabled();

interface RegionStats {
  commits: number;
  /** Commits whose phase was 'update' (a re-render, not the initial mount). */
  updates: number;
  totalActualMs: number;
  maxActualMs: number;
  lastCommitTime: number;
}

const stats = new Map<string, RegionStats>();

const onRender: ProfilerOnRenderCallback = (id, phase, actualDuration, _base, _start, commitTime) => {
  let s = stats.get(id);
  if (!s) {
    s = { commits: 0, updates: 0, totalActualMs: 0, maxActualMs: 0, lastCommitTime: 0 };
    stats.set(id, s);
  }
  s.commits += 1;
  if (phase === 'update') s.updates += 1;
  s.totalActualMs += actualDuration;
  if (actualDuration > s.maxActualMs) s.maxActualMs = actualDuration;
  s.lastCommitTime = commitTime;
};

/**
 * Wrap a subtree to measure it. `id` names the region in the dump (e.g.
 * "MessageList", "StateBar"). When profiling is disabled this is a no-op
 * passthrough — no <Profiler>, no measurement, no global side effects.
 */
export function RenderProfiler({ id, children }: { id: string; children: ReactNode }) {
  if (!ENABLED) return <>{children}</>;
  return (
    <Profiler id={id} onRender={onRender}>
      {children}
    </Profiler>
  );
}

// ---------------------------------------------------------------------------
// Console API (DEV only). Installed even when the flag is off so enable() is
// reachable without first knowing the localStorage key.
// ---------------------------------------------------------------------------

interface ProfilerApi {
  enabled: boolean;
  enable: () => void;
  disable: () => void;
  reset: () => void;
  dump: () => void;
  /** Machine-readable counterpart to dump() — returns the per-region stats as
   *  a plain object so an automated driver can read them via page.evaluate. */
  snapshot: () => Record<string, { commits: number; updates: number; avgMs: number; maxMs: number; totalMs: number }>;
  auto: (intervalMs?: number) => void;
  stop: () => void;
}

if (import.meta.env.DEV && typeof window !== 'undefined') {
  let autoTimer: ReturnType<typeof setInterval> | null = null;

  const dump = () => {
    if (stats.size === 0) {
      console.info(
        ENABLED
          ? '[profiler] no commits recorded yet'
          : '[profiler] disabled — run __phoenixProfiler.enable() and reload',
      );
      return;
    }
    const rows: Record<string, { commits: number; updates: number; 'avg ms': number; 'max ms': number; 'total ms': number }> = {};
    for (const [id, s] of stats) {
      rows[id] = {
        commits: s.commits,
        updates: s.updates,
        'avg ms': Number((s.totalActualMs / s.commits).toFixed(2)),
        'max ms': Number(s.maxActualMs.toFixed(2)),
        'total ms': Number(s.totalActualMs.toFixed(1)),
      };
    }
    console.table(rows);
  };

  const api: ProfilerApi = {
    enabled: ENABLED,
    enable: () => {
      try { localStorage.setItem(FLAG_KEY, '1'); } catch { /* ignore */ }
      location.reload();
    },
    disable: () => {
      try { localStorage.removeItem(FLAG_KEY); } catch { /* ignore */ }
      location.reload();
    },
    reset: () => stats.clear(),
    dump,
    snapshot: () => {
      const out: Record<string, { commits: number; updates: number; avgMs: number; maxMs: number; totalMs: number }> = {};
      for (const [id, s] of stats) {
        out[id] = {
          commits: s.commits,
          updates: s.updates,
          avgMs: Number((s.totalActualMs / s.commits).toFixed(3)),
          maxMs: Number(s.maxActualMs.toFixed(3)),
          totalMs: Number(s.totalActualMs.toFixed(2)),
        };
      }
      return out;
    },
    auto: (intervalMs = 2000) => {
      if (autoTimer) clearInterval(autoTimer);
      autoTimer = setInterval(dump, intervalMs);
      console.info(`[profiler] auto-dump every ${intervalMs}ms — __phoenixProfiler.stop() to halt`);
    },
    stop: () => {
      if (autoTimer) { clearInterval(autoTimer); autoTimer = null; }
    },
  };

  (window as unknown as { __phoenixProfiler: ProfilerApi }).__phoenixProfiler = api;
}
