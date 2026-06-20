/**
 * Pure work-scope helpers shared by both surfaces (`WorkScopeSection` in the
 * left panel and the standalone `WorkScopePanel` chain dock). Kept out of the
 * component file so the component module stays component-only for React Fast
 * Refresh (specs/work-scope-ui/, REQ-WSUI-009 / REQ-WSUI-010).
 */

import type { WorkScopeInventory, BashHandleState } from '../api';

type BrowserSession = NonNullable<WorkScopeInventory['browser']>;

/** A live browser session is "idle" once it has gone this long without
 *  activity. The single definition of the idle cutoff — the collapsed summary,
 *  the live count, and the expanded browser row all read it, so they cannot
 *  disagree at the boundary (an off-by-one `>` vs `>=` once split them). */
export const BROWSER_IDLE_THRESHOLD_MS = 60_000;

/** Whether a browser session reads as idle: live on the wire but quiet past the
 *  threshold. "idle" is a purely client-side presentation over `idle_ms`. */
export function isBrowserIdle(browser: BrowserSession): boolean {
  return browser.state === 'live' && browser.idle_ms >= BROWSER_IDLE_THRESHOLD_MS;
}

/** A bash handle that reads as "running right now". */
export function isLive(state: BashHandleState): boolean {
  return state === 'running' || state === 'kill_pending_kernel';
}

/** Whether any bash handle reads as "running right now" (running or
 *  kill_pending_kernel). Gates the running-handle inventory poll: byte counts
 *  only grow while a handle is live, so once nothing is running the poll stops. */
export function hasRunningBash(inv: WorkScopeInventory | null): boolean {
  return inv != null && inv.bash.some((h) => isLive(h.state));
}

/** Count of resources that read as "running right now": live bash handles
 *  (running + kill_pending_kernel) plus a live, non-idle browser session.
 *  Drives the collapsed-rail badge. An idle browser is excluded so it is not
 *  both counted as "live" and separately labelled "browser idle" in the
 *  summary — it appears only as the idle label. */
export function workScopeLiveCount(inv: WorkScopeInventory | null): number {
  if (!inv) return 0;
  const liveBash = inv.bash.filter((h) => isLive(h.state)).length;
  const liveBrowser = inv.browser && inv.browser.state === 'live' && !isBrowserIdle(inv.browser) ? 1 : 0;
  return liveBash + liveBrowser;
}

/** Whether the scope owns ANY live resource whose displayed fields can advance
 *  between SSE pushes — so the panel should keep polling.
 *
 *  Broader than {@link hasRunningBash}: the `work_scope_update` push is
 *  edge-triggered on state transitions, but some live fields drift continuously
 *  with no dedicated edge (a browser session's `idle_ms`), and a tmux entry
 *  created off the conversation's own SSE channel (e.g. by opening the terminal
 *  panel) is belt-and-suspenders covered here. True when:
 *    - any bash handle is running / kill_pending_kernel, OR
 *    - a tmux server entry exists and is live or not-yet-probed
 *      (i.e. a server exists; a `gone` entry is terminal and need not poll), OR
 *    - a browser session is live.
 *  Self-limiting by construction: once nothing matches, the poll stops. */
export function hasLiveResource(inv: WorkScopeInventory | null): boolean {
  if (inv == null) return false;
  if (inv.bash.some((h) => isLive(h.state))) return true;
  if (inv.tmux != null && (inv.tmux.status === 'live' || inv.tmux.status === 'not_probed'))
    return true;
  if (inv.browser?.state === 'live') return true;
  return false;
}
