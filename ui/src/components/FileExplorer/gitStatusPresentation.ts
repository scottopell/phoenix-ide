import type { ConversationGitStatusResponse } from '../../api';

type Snapshot = Extract<ConversationGitStatusResponse, { kind: 'snapshot' }>;

export function checkoutLabel(status: Snapshot['checkout_status']): string {
  switch (status.kind) {
    case 'named_branch': {
      const remote = status.remote_status;
      if (remote.kind === 'tracked' || remote.kind === 'matching') {
        const relationship = remote.ahead === 0 && remote.behind === 0
          ? 'up to date'
          : [remote.ahead > 0 ? `↑${remote.ahead}` : '', remote.behind > 0 ? `↓${remote.behind}` : ''].filter(Boolean).join(' ');
        const relationshipLabel = remote.kind === 'matching'
          ? `matching ${remote.remote_ref} (not upstream)`
          : remote.remote_ref;
        return `${status.branch_name} · ${relationshipLabel} · ${relationship}`;
      }
      if (remote.kind === 'unavailable') return `${status.branch_name} · upstream unavailable`;
      return status.branch_name;
    }
    case 'detached': return `detached @ ${status.head_oid.slice(0, 7)}`;
    case 'unborn': return `${status.branch_name} · no commits`;
    case 'unavailable': return 'checkout unavailable';
  }
}
