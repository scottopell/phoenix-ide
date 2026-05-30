import { describe, it, expect } from 'vitest';
import { effectiveWorkflow, type NewConversationWorkflow } from './useCreateConversation';

describe('effectiveWorkflow', () => {
  it('is direct when the directory is not a git repo', () => {
    expect(effectiveWorkflow(null, false, null)).toEqual({ kind: 'direct' });
    // An override is ignored until git is confirmed.
    expect(effectiveWorkflow({ kind: 'continueBranch', branch: 'x' }, false, 'main'))
      .toEqual({ kind: 'direct' });
  });

  it('is direct while git status is still unknown', () => {
    expect(effectiveWorkflow(null, null, null)).toEqual({ kind: 'direct' });
  });

  it('defaults to the fresh-worktree workflow the instant a git repo is detected', () => {
    // Regression: no 'direct' flash. Before branch metadata loads (fallback
    // null) the default is already planFromBranch, not direct.
    expect(effectiveWorkflow(null, true, null)).toEqual({ kind: 'planFromBranch', baseBranch: null });
    // Once the default branch resolves it fills in without any state churn.
    expect(effectiveWorkflow(null, true, 'main')).toEqual({ kind: 'planFromBranch', baseBranch: 'main' });
  });

  it('honors an explicit direct choice even in a git repo', () => {
    expect(effectiveWorkflow({ kind: 'direct' }, true, 'main')).toEqual({ kind: 'direct' });
  });

  it('fills a null override branch from the default branch, but keeps an explicit one', () => {
    expect(effectiveWorkflow({ kind: 'planFromBranch', baseBranch: null }, true, 'main'))
      .toEqual({ kind: 'planFromBranch', baseBranch: 'main' });
    expect(effectiveWorkflow({ kind: 'planFromBranch', baseBranch: 'feature' }, true, 'main'))
      .toEqual({ kind: 'planFromBranch', baseBranch: 'feature' });
    expect(effectiveWorkflow({ kind: 'continueBranch', branch: null }, true, 'main'))
      .toEqual({ kind: 'continueBranch', branch: 'main' });
    const task = { kind: 'planFromTask', task: null, baseBranch: null } satisfies NewConversationWorkflow;
    expect(effectiveWorkflow(task, true, 'main')).toEqual({ kind: 'planFromTask', task: null, baseBranch: 'main' });
  });
});
