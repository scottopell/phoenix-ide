import { describe, it, expect } from 'vitest';
import type { Project } from '../api';
import { effectiveWorkflow, suggestedProjectDirs, type NewConversationWorkflow } from './useCreateConversation';

describe('effectiveWorkflow', () => {
  it('is direct when the directory is not a git repo', () => {
    expect(effectiveWorkflow(null, false, null, false)).toEqual({ kind: 'direct' });
    // An override is ignored until git is confirmed.
    expect(effectiveWorkflow({ kind: 'continueBranch', branch: 'x' }, false, 'main', false))
      .toEqual({ kind: 'direct' });
  });

  it('is direct while git status is still unknown', () => {
    expect(effectiveWorkflow(null, null, null, false)).toEqual({ kind: 'direct' });
  });

  it('defaults to the fresh-worktree workflow the instant a git repo is detected', () => {
    // Regression: no 'direct' flash. Before branch metadata loads (fallback
    // null, fetch not settled) the default is already planFromBranch, not direct.
    expect(effectiveWorkflow(null, true, null, false)).toEqual({ kind: 'planFromBranch', baseBranch: null });
    // Once the default branch resolves it fills in without any state churn.
    expect(effectiveWorkflow(null, true, 'main', false)).toEqual({ kind: 'planFromBranch', baseBranch: 'main' });
  });

  it('falls back to direct for an unborn/branchless repo once the fetch settles', () => {
    // Regression: a repo with no resolvable branch must not get stuck on a
    // branchless planFromBranch (Send permanently disabled). Only the settled
    // signal (branchUnavailable) triggers this — a pending fetch (false) does not.
    expect(effectiveWorkflow(null, true, null, true)).toEqual({ kind: 'direct' });
    // An explicit non-direct choice is still honored even with no branch.
    expect(effectiveWorkflow({ kind: 'planFromBranch', baseBranch: null }, true, null, true))
      .toEqual({ kind: 'planFromBranch', baseBranch: null });
  });

  it('honors an explicit direct choice even in a git repo', () => {
    expect(effectiveWorkflow({ kind: 'direct' }, true, 'main', false)).toEqual({ kind: 'direct' });
  });

  it('fills a null override branch from the default branch, but keeps an explicit one', () => {
    expect(effectiveWorkflow({ kind: 'planFromBranch', baseBranch: null }, true, 'main', false))
      .toEqual({ kind: 'planFromBranch', baseBranch: 'main' });
    expect(effectiveWorkflow({ kind: 'planFromBranch', baseBranch: 'feature' }, true, 'main', false))
      .toEqual({ kind: 'planFromBranch', baseBranch: 'feature' });
    expect(effectiveWorkflow({ kind: 'continueBranch', branch: null }, true, 'main', false))
      .toEqual({ kind: 'continueBranch', branch: 'main' });
    const task = { kind: 'planFromTask', task: null, baseBranch: null } satisfies NewConversationWorkflow;
    expect(effectiveWorkflow(task, true, 'main', false)).toEqual({ kind: 'planFromTask', task: null, baseBranch: 'main' });
  });
});

function project(canonical_path: string, conversation_count: number, created_at: string): Project {
  return { id: canonical_path, canonical_path, main_ref: 'main', created_at, conversation_count };
}

describe('suggestedProjectDirs', () => {
  it('prefers commonly used projects and uses recency to break ties', () => {
    expect(suggestedProjectDirs([
      project('/projects/quiet-new', 1, '2026-07-14T00:00:00Z'),
      project('/projects/common-old', 8, '2026-07-10T00:00:00Z'),
      project('/projects/common-new', 8, '2026-07-12T00:00:00Z'),
      project('/projects/mid', 4, '2026-07-13T00:00:00Z'),
    ])).toEqual([
      '/projects/common-new',
      '/projects/common-old',
      '/projects/mid',
      '/projects/quiet-new',
    ]);
  });

  it('limits suggestions to five projects', () => {
    const projects = Array.from({ length: 7 }, (_, index) => (
      project(`/projects/${index}`, index, `2026-07-${String(index + 1).padStart(2, '0')}T00:00:00Z`)
    ));

    expect(suggestedProjectDirs(projects)).toHaveLength(5);
  });
});
