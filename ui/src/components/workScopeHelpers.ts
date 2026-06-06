/**
 * Pure work-scope helpers shared by both surfaces (`WorkScopeSection` in the
 * left panel and the standalone `WorkScopePanel` chain dock). Kept out of the
 * component file so the component module stays component-only for React Fast
 * Refresh (specs/work-scope-ui/, REQ-WSUI-009 / REQ-WSUI-010).
 */

import type { WorkScopeInventory, BashHandleState } from '../api';

/** A bash handle that reads as "running right now". */
export function isLive(state: BashHandleState): boolean {
  return state === 'running' || state === 'kill_pending_kernel';
}

/** Count of resources that read as "running right now": live bash handles
 *  (running + kill_pending_kernel) plus a live (non-idle) browser session.
 *  Drives the collapsed-rail badge. */
export function workScopeLiveCount(inv: WorkScopeInventory | null): number {
  if (!inv) return 0;
  const liveBash = inv.bash.filter((h) => isLive(h.state)).length;
  const liveBrowser = inv.browser && inv.browser.state === 'live' ? 1 : 0;
  return liveBash + liveBrowser;
}
