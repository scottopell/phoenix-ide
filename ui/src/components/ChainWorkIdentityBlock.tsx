// The "what unit of work is this" facet of the chain page's work-scope dock
// (REQ-CHN-008). The work identity (worktree / branch / base / task) is durable
// and arrives on the `ChainView` from the chain's `ConvMode` git metadata; PR
// health rides the per-conversation PR-status pipeline that drives the StateBar,
// keyed by the worktree-owning member, so it stays off the chain page's load
// path. When the chain owns no managed work scope the block says so rather than
// rendering empty fields.

import type { ChainWorkIdentity } from '../api';
import { useConversationPrStatus } from '../hooks/useConversationPrStatus';
import {
  prBadgeClass,
  prBadgeLabel,
  prTooltip,
  prFeedbackFreshnessLabel,
} from './prBadge';

export function ChainWorkIdentityBlock({ identity }: { identity: ChainWorkIdentity | null }) {
  // Work mode carries a task; Branch mode does not — the PR-status pipeline
  // only enables for those two modes, which is exactly when `identity` is
  // present. A null id keeps the hook disabled (no fetch) for the empty state.
  const prHandle = useConversationPrStatus({
    conversationId: identity?.work_conv_id ?? null,
    convModeLabel: identity ? (identity.task_id ? 'Work' : 'Branch') : undefined,
    branchName: identity?.branch_name ?? null,
  });

  if (!identity) {
    return (
      <section className="chain-work-identity chain-work-identity--empty">
        <span className="chain-work-identity-title">Work identity</span>
        <span className="chain-work-identity-empty">No managed work scope</span>
      </section>
    );
  }

  const pr = prHandle.state.status === 'ready' ? prHandle.state.prStatus : null;
  const freshness = pr?.found ? prFeedbackFreshnessLabel(pr) : null;

  return (
    <section className="chain-work-identity">
      <span className="chain-work-identity-title">Work identity</span>
      <dl className="chain-work-identity-fields">
        <div className="chain-work-identity-field">
          <dt>Branch</dt>
          <dd title={`${identity.branch_name} → ${identity.base_branch}`}>
            {identity.branch_name} <span className="chain-work-identity-arrow">→</span>{' '}
            {identity.base_branch}
          </dd>
        </div>
        {identity.task_id && (
          <div className="chain-work-identity-field">
            <dt>Task</dt>
            <dd title={identity.task_title ?? identity.task_id}>
              <span className="chain-work-identity-task-id">{identity.task_id}</span>
              {identity.task_title ? ` ${identity.task_title}` : ''}
            </dd>
          </div>
        )}
        <div className="chain-work-identity-field">
          <dt>Worktree</dt>
          <dd className="chain-work-identity-path" title={identity.worktree_path}>
            {identity.worktree_path}
          </dd>
        </div>
        <div className="chain-work-identity-field">
          <dt>PR</dt>
          <dd>
            {pr?.found ? (
              <span className="chain-work-identity-pr">
                <span className={prBadgeClass(pr)} title={prTooltip(pr)}>
                  {prBadgeLabel(pr)}
                </span>
                {freshness && (
                  <span className="chain-work-identity-pr-freshness">{freshness}</span>
                )}
              </span>
            ) : (
              <span className="chain-work-identity-muted">
                {prHandle.state.status === 'loading' ? '…' : 'no PR'}
              </span>
            )}
          </dd>
        </div>
      </dl>
    </section>
  );
}
