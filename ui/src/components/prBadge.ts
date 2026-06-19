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

/** Short feedback-freshness tag (`"3 new"`, `"1 updated"`), or null
 *  when there is no freshness signal to show. When the fetch was degraded
 *  (`feedback_coverage` present) the count is a lower bound, so it's prefixed
 *  with "at least". */
export function prFeedbackFreshnessLabel(pr: PrStatusResponse): string | null {
  const freshness = pr.feedback_freshness;
  if (!freshness) return null;
  const floor = pr.feedback_coverage ? 'at least ' : '';
  if (freshness.state === 'new') {
    return `${floor}${freshness.count} new`;
  }
  return `${floor}${freshness.count} updated`;
}

export interface PrFeedbackCoverageMarker {
  /** True for an actionable auth failure (the user can run `gh auth login`). */
  actionable: boolean;
  /** Short visible text; empty for the low-key transient case (icon only). */
  label: string;
  tooltip: string;
}

const SURFACE_LABELS: Record<string, string> = {
  issue_comments: 'issue comments',
  review_comments: 'review comments',
  review_summaries: 'review summaries',
  review_threads: 'review threads',
};

/** Warning marker for a degraded feedback fetch, or null when coverage is
 *  complete. Distinct from the freshness label so an incomplete fetch is never
 *  rendered as a content change. Auth failures are actionable and labelled;
 *  transient gaps are icon-only with the reason in the tooltip. */
export function prFeedbackCoverageMarker(pr: PrStatusResponse): PrFeedbackCoverageMarker | null {
  const coverage = pr.feedback_coverage;
  if (!coverage) return null;
  const surfaces = coverage.surfaces.map((s) => SURFACE_LABELS[s] ?? s).join(', ');
  if (coverage.kind === 'auth_required') {
    return {
      actionable: true,
      label: 'GitHub sign-in needed',
      tooltip: `Couldn't read ${surfaces} — the GitHub CLI isn't authenticated. Run \`gh auth login\`, then refresh.`,
    };
  }
  return {
    actionable: false,
    label: '',
    tooltip: `Feedback may be incomplete — couldn't fetch ${surfaces}.`,
  };
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
