// Shared PR-status presentation helpers.
//
// Pure formatting over a `PrStatusResponse` (the per-conversation PR-status
// pipeline, `specs/projects/` REQ-PROJ-011/030/031). Single source of truth so
// every surface that shows PR health — the StateBar, the PR remediation
// actions, and the chain page's work-identity dock (REQ-CHN-008) — renders the
// same badge, label, and freshness text rather than each re-deriving its own.

import type { PrStatusResponse } from '../api';

export function prBadgeClass(pr: PrStatusResponse): string {
  if (pr.display_state === 'merged') return 'pr-badge pr-badge--merged';
  if (pr.display_state === 'closed') return 'pr-badge pr-badge--failing';
  if (pr.display_state === 'draft') return 'pr-badge pr-badge--pending';
  switch (pr.check_state) {
    case 'passing': return 'pr-badge pr-badge--passing';
    case 'failing': return 'pr-badge pr-badge--failing';
    case 'pending': return 'pr-badge pr-badge--pending';
    default: return 'pr-badge pr-badge--unknown';
  }
}

export function prBadgeLabel(pr: PrStatusResponse): string {
  const n = pr.number ? `#${pr.number}` : 'PR';
  if (pr.display_state === 'merged') return `${n} merged`;
  if (pr.display_state === 'closed') return `${n} closed`;
  if (pr.display_state === 'draft') return `${n} draft`;
  if (pr.check_state === 'passing') return `${n} checks ✓`;
  if (pr.check_state === 'failing') return `${n} checks ✗`;
  if (pr.check_state === 'pending') return `${n} checks ...`;
  return n;
}

export function prRefreshStaleText(pr: PrStatusResponse): string {
  if (pr.refresh.state === 'not_found') return 'no PR found for current branch';
  if (pr.refresh.state === 'unavailable') return `refresh unavailable (${pr.refresh.reason ?? 'unknown'})`;
  return 'refresh did not produce fresh PR data';
}

export function prTooltip(pr: PrStatusResponse): string {
  const label = pr.number ? `PR #${pr.number}` : 'PR';
  const title = pr.title ? ` — ${pr.title}` : '';
  const state = pr.display_state ?? 'unknown';
  const checks = pr.check_state ?? 'unknown';
  const freshness = pr.refresh.stale
    ? `
Refresh: stale (${prRefreshStaleText(pr)})`
    : '';
  return `${label}${title}
State: ${state}
Checks: ${checks}${freshness}`;
}

/** Short feedback-freshness tag (`"3 new"`, `"1 comment updated"`), or null
 *  when there is no freshness signal to show. */
export function prFeedbackFreshnessLabel(pr: PrStatusResponse): string | null {
  const freshness = pr.feedback_freshness;
  if (!freshness) return null;
  if (freshness.state === 'new') {
    return `${freshness.count} new`;
  }
  return freshness.count === 1 ? '1 comment updated' : `${freshness.count} comments updated`;
}

/** Short hint for why PR status is unavailable (the pipeline couldn't check),
 *  or null when the reason is absent — distinct from a genuine "no PR found"
 *  so a surface can avoid telling the user there's no PR when it simply
 *  couldn't look. */
export function unavailablePrHint(
  reason: PrStatusResponse['unavailable_reason'],
): string | null {
  switch (reason) {
    case 'gh_missing': return 'gh missing';
    case 'not_authenticated': return 'gh auth';
    case 'not_git_repo': return 'no worktree';
    case 'command_failed': return 'PR status unavailable';
    default: return null;
  }
}
