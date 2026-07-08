import { describe, expect, it } from 'vitest';
import type {
  PrStatusResponse,
  PrDisplayState,
  PrCheckState,
  PrFeedbackFreshness,
  PrRefreshState,
} from '../api';
import {
  deriveWorkDisposition,
  type WorkDispositionInput,
  type WorkDisposition,
} from './workDisposition';

// --- Fixtures -------------------------------------------------------------

const PR_URL = 'https://github.com/acme/repo/pull/42';
const PR_NUMBER = 42;

function refresh(state: PrRefreshState): PrStatusResponse['refresh'] {
  return {
    state,
    last_attempted_at: '2026-01-01T00:00:00Z',
    stale: false,
  };
}

function loadingWorkChange(): PrStatusResponse['work_change'] {
  return { kind: 'loading' };
}

function cleanWorkChange(): PrStatusResponse['work_change'] {
  return { kind: 'clean' };
}

/** A found PR in a given display state, refresh fresh, no checks/feedback. */
function foundPr(
  display_state: PrDisplayState,
  extra: Partial<PrStatusResponse> = {},
): PrStatusResponse {
  return {
    found: true,
    number: PR_NUMBER,
    url: PR_URL,
    display_state,
    refresh: refresh('fresh'),
    work_change: cleanWorkChange(),
    ...extra,
  };
}

/** No PR found; refresh state controls gh-availability. */
function notFound(refreshState: PrRefreshState, extra: Partial<PrStatusResponse> = {}): PrStatusResponse {
  return {
    found: false,
    refresh: refresh(refreshState),
    work_change: cleanWorkChange(),
    ...extra,
  };
}

function input(overrides: Partial<WorkDispositionInput> = {}): WorkDispositionInput {
  return {
    convModeLabel: 'Work',
    phaseType: 'idle',
    continuedInConvId: null,
    prStatus: null,
    prLoading: false,
    workChange: cleanWorkChange(),
    canSendMessage: true,
    ...overrides,
  };
}

// --- Visibility -----------------------------------------------------------

describe('visibility (REQ-WAB-001)', () => {
  it('hidden when mode is neither Work nor Branch', () => {
    const d = deriveWorkDisposition(input({ convModeLabel: 'Explore', prStatus: foundPr('merged') }));
    expect(d.visible).toBe(false);
    expect(d.primary).toBe('none');
  });

  it('hidden when convModeLabel is undefined', () => {
    const d = deriveWorkDisposition(input({ convModeLabel: undefined }));
    expect(d.visible).toBe(false);
  });

  it('hidden when phase is not disposable (e.g. awaiting_llm)', () => {
    const d = deriveWorkDisposition(input({ phaseType: 'awaiting_llm', prStatus: foundPr('merged') }));
    expect(d.visible).toBe(false);
    expect(d.primary).toBe('none');
  });

  it('visible for Branch mode in idle phase', () => {
    const d = deriveWorkDisposition(input({ convModeLabel: 'Branch', prStatus: notFound('fresh') }));
    expect(d.visible).toBe(true);
  });

  it('visible for each disposable phase', () => {
    for (const phaseType of ['idle', 'error', 'context_exhausted']) {
      const d = deriveWorkDisposition(input({ phaseType, prStatus: notFound('fresh') }));
      expect(d.visible).toBe(true);
    }
  });
});

// --- Row 1: continued -----------------------------------------------------

describe('continued (REQ-WAB-009)', () => {
  it('no primary, all terminal verbs hidden, continued note only', () => {
    const d = deriveWorkDisposition(input({ continuedInConvId: 'conv-123', prStatus: foundPr('merged') }));
    expect(d.visible).toBe(true);
    expect(d.primary).toBe('none');
    expect(d.resolve).toBeNull();
    expect(d.showCleanUp).toBe(false);
    expect(d.showAbandon).toBe(false);
    expect(d.note?.kind).toBe('continued');
  });

  it('continued wins over stuck phase', () => {
    const d = deriveWorkDisposition(
      input({ phaseType: 'error', continuedInConvId: 'conv-9', prStatus: foundPr('open') }),
    );
    expect(d.primary).toBe('none');
    expect(d.note?.kind).toBe('continued');
  });
});

// --- Row 2: checking / loading --------------------------------------------

describe('checking / loading', () => {
  it('abandon primary, no clean up, checking note, abandon shown', () => {
    const d = deriveWorkDisposition(input({ prLoading: true, prStatus: null }));
    expect(d.visible).toBe(true);
    expect(d.primary).toBe('abandon');
    expect(d.resolve).toBeNull();
    expect(d.showCleanUp).toBe(false);
    expect(d.showAbandon).toBe(true);
    expect(d.note?.kind).toBe('checking');
  });

  it('loading=true but a usable prStatus already present does NOT show checking', () => {
    const d = deriveWorkDisposition(input({ prLoading: true, prStatus: foundPr('merged') }));
    expect(d.note?.kind).not.toBe('checking');
    expect(d.primary).toBe('clean_up');
  });
});

// --- Row 3: stuck sub-cases -----------------------------------------------

describe('stuck — RESOLVE always suppressed (REQ-WAB-005)', () => {
  it('merged → clean_up primary, clean up shown', () => {
    const d = deriveWorkDisposition(input({ phaseType: 'error', prStatus: foundPr('merged') }));
    expect(d.primary).toBe('clean_up');
    expect(d.resolve).toBeNull();
    expect(d.showCleanUp).toBe(true);
    expect(d.note).toBeNull();
  });

  it('closed → abandon primary, BOTH terminal verbs shown, pr_closed note', () => {
    const d = deriveWorkDisposition(input({ phaseType: 'context_exhausted', prStatus: foundPr('closed') }));
    expect(d.primary).toBe('abandon');
    // Stuck keeps Clean up available even when Abandon is primary (codex A / DispositionStuck).
    expect(d.showCleanUp).toBe(true);
    expect(d.showAbandon).toBe(true);
    expect(d.note?.kind).toBe('pr_closed');
  });

  it('open → abandon primary, BOTH terminal verbs shown, pr_open_stuck note', () => {
    const d = deriveWorkDisposition(input({ phaseType: 'error', prStatus: foundPr('open') }));
    expect(d.primary).toBe('abandon');
    expect(d.resolve).toBeNull();
    expect(d.showCleanUp).toBe(true);
    expect(d.showAbandon).toBe(true);
    expect(d.note?.kind).toBe('pr_open_stuck');
  });

  it('draft → abandon primary, pr_open_stuck note', () => {
    const d = deriveWorkDisposition(input({ phaseType: 'error', prStatus: foundPr('draft') }));
    expect(d.primary).toBe('abandon');
    expect(d.note?.kind).toBe('pr_open_stuck');
  });

  it('gh unavailable → clean_up primary, gh_unavailable note', () => {
    const d = deriveWorkDisposition(
      input({ phaseType: 'error', prStatus: notFound('unavailable', { unavailable_reason: 'gh_missing' }) }),
    );
    expect(d.primary).toBe('clean_up');
    expect(d.showCleanUp).toBe(true);
    expect(d.note?.kind).toBe('gh_unavailable');
  });

  it('no PR, refresh ok → clean_up primary, no note', () => {
    const d = deriveWorkDisposition(input({ phaseType: 'context_exhausted', prStatus: notFound('fresh') }));
    expect(d.primary).toBe('clean_up');
    expect(d.showCleanUp).toBe(true);
    expect(d.note).toBeNull();
  });
});

// --- Row 4: idle open/draft RESOLVE ---------------------------------------

describe('idle open/draft — RESOLVE', () => {
  const fresh: PrFeedbackFreshness = { state: 'new', count: 2 };

  it('address_feedback when open + failing checks + canSendMessage; Open PR rides secondary', () => {
    const d = deriveWorkDisposition(
      input({ prStatus: foundPr('open', { check_state: 'failing' as PrCheckState }) }),
    );
    expect(d.primary).toBe('resolve');
    expect(d.resolve).toEqual({ kind: 'address_feedback' });
    expect(d.secondaryResolve).toEqual({ kind: 'open_pr', url: PR_URL, number: PR_NUMBER });
    expect(d.showCleanUp).toBe(false);
    expect(d.showAbandon).toBe(true);
  });

  it('open + passing checks → Address feedback primary, Merge rides as a secondary link', () => {
    const d = deriveWorkDisposition(
      input({ prStatus: foundPr('open', { check_state: 'passing' as PrCheckState }) }),
    );
    // Review comments may need addressing on a green PR, so Address feedback is
    // the primary on every reachable open PR; the honest Merge link rides
    // alongside as a non-glowing secondary (REQ-WAB-003 — never a 2nd primary).
    expect(d.primary).toBe('resolve');
    expect(d.resolve).toEqual({ kind: 'address_feedback' });
    expect(d.secondaryResolve).toEqual({ kind: 'merge_pr', url: PR_URL, number: PR_NUMBER });
  });

  it('coverage gap is orthogonal — passing + coverage-only still Address primary + Merge secondary', () => {
    const d = deriveWorkDisposition(
      input({
        prStatus: foundPr('open', {
          check_state: 'passing' as PrCheckState,
          feedback_coverage: { kind: 'auth_required', surfaces: ['review_threads'] },
        }),
      }),
    );
    // A coverage gap does NOT change which verb shows; the coverage marker rides
    // on the resolve verb (rendered by the component), not into routing.
    expect(d.primary).toBe('resolve');
    expect(d.resolve).toEqual({ kind: 'address_feedback' });
    expect(d.secondaryResolve).toEqual({ kind: 'merge_pr', url: PR_URL, number: PR_NUMBER });
  });

  it('stale/unavailable refresh on an addressable open PR keeps Address feedback primary', () => {
    const d = deriveWorkDisposition(
      input({
        prStatus: foundPr('open', {
          check_state: 'passing' as PrCheckState,
          refresh: { state: 'unavailable', last_attempted_at: '2026-01-01T00:00:00Z', stale: true },
          unavailable_reason: 'command_failed',
        }),
      }),
    );
    // A cached/open PR should not first render Open PR and then flip to Address
    // feedback when the async refresh completes. The safe link-out still rides as
    // secondary while mergeability is unconfirmed.
    expect(d.primary).toBe('resolve');
    expect(d.resolve).toEqual({ kind: 'address_feedback' });
    expect(d.secondaryResolve).toEqual({ kind: 'open_pr', url: PR_URL, number: PR_NUMBER });
    expect(d.showCleanUp).toBe(false);
    expect(d.showAbandon).toBe(true);
  });

  it('address_feedback when open + fresh feedback present', () => {
    const d = deriveWorkDisposition(
      input({ prStatus: foundPr('open', { feedback_freshness: fresh }) }),
    );
    expect(d.resolve).toEqual({ kind: 'address_feedback' });
  });

  it('NOT addressable when canSendMessage is false → falls through to open_pr', () => {
    const d = deriveWorkDisposition(
      input({ canSendMessage: false, prStatus: foundPr('open', { check_state: 'failing' as PrCheckState }) }),
    );
    expect(d.resolve).toEqual({ kind: 'open_pr', url: PR_URL, number: PR_NUMBER });
  });

  it('refresh unavailable on a found open PR still addressable when a message channel exists', () => {
    const d = deriveWorkDisposition(
      input({
        prStatus: foundPr('open', {
          check_state: 'failing' as PrCheckState,
          refresh: refresh('unavailable'),
        }),
      }),
    );
    expect(d.resolve).toEqual({ kind: 'address_feedback' });
    expect(d.secondaryResolve).toEqual({ kind: 'open_pr', url: PR_URL, number: PR_NUMBER });
  });

  it('merge_pr as primary when open + passing but no message channel (canSendMessage false)', () => {
    const d = deriveWorkDisposition(
      input({ canSendMessage: false, prStatus: foundPr('open', { check_state: 'passing' as PrCheckState }) }),
    );
    // No channel to post an auto-fix message → Address feedback unreachable, so
    // the green PR routes to Merge as the primary (no secondary).
    expect(d.primary).toBe('resolve');
    expect(d.resolve).toEqual({ kind: 'merge_pr', url: PR_URL, number: PR_NUMBER });
    expect(d.secondaryResolve).toBeNull();
  });

  it('open + pending checks → Address feedback primary, Open PR secondary (pending ≠ green)', () => {
    const d = deriveWorkDisposition(
      input({ prStatus: foundPr('open', { check_state: 'pending' as PrCheckState }) }),
    );
    expect(d.resolve).toEqual({ kind: 'address_feedback' });
    expect(d.secondaryResolve).toEqual({ kind: 'open_pr', url: PR_URL, number: PR_NUMBER });
  });

  it('open_pr (never merge) for a draft even with passing checks', () => {
    const d = deriveWorkDisposition(
      input({ prStatus: foundPr('draft', { check_state: 'passing' as PrCheckState }) }),
    );
    // Drafts are not addressable and never advertise a Merge.
    expect(d.resolve).toEqual({ kind: 'open_pr', url: PR_URL, number: PR_NUMBER });
    expect(d.secondaryResolve).toBeNull();
  });

  it('open PR missing url/number but addressable → Address feedback, no secondary', () => {
    const pr = foundPr('open', { check_state: 'passing' as PrCheckState });
    delete pr.url;
    delete pr.number;
    const d = deriveWorkDisposition(input({ prStatus: pr }));
    // Address feedback needs no PR url (the auto-fix message is built from the
    // conversation), so a missing link does not block it; the Merge secondary
    // simply does not appear.
    expect(d.primary).toBe('resolve');
    expect(d.resolve).toEqual({ kind: 'address_feedback' });
    expect(d.secondaryResolve).toBeNull();
  });

  it('open PR missing url/number AND no message channel → safe abandon fallback', () => {
    const pr = foundPr('open', { check_state: 'passing' as PrCheckState });
    delete pr.url;
    delete pr.number;
    const d = deriveWorkDisposition(input({ canSendMessage: false, prStatus: pr }));
    expect(d.primary).toBe('abandon');
    expect(d.resolve).toBeNull();
  });
});

// --- Rows 5-8: idle terminal ----------------------------------------------

describe('idle terminal dispositions', () => {
  it('merged → clean_up primary', () => {
    const d = deriveWorkDisposition(input({ prStatus: foundPr('merged') }));
    expect(d.primary).toBe('clean_up');
    expect(d.showCleanUp).toBe(true);
    expect(d.note).toBeNull();
  });

  it('closed → abandon primary, pr_closed note', () => {
    const d = deriveWorkDisposition(input({ prStatus: foundPr('closed') }));
    expect(d.primary).toBe('abandon');
    expect(d.showCleanUp).toBe(false);
    expect(d.note?.kind).toBe('pr_closed');
  });

  it('gh unavailable → clean_up primary, gh_unavailable note', () => {
    const d = deriveWorkDisposition(
      input({ prStatus: notFound('unavailable', { unavailable_reason: 'not_authenticated' }) }),
    );
    expect(d.primary).toBe('clean_up');
    expect(d.showCleanUp).toBe(true);
    expect(d.note?.kind).toBe('gh_unavailable');
  });

  it('gh unavailable via unavailable_reason even when refresh state is fresh', () => {
    const d = deriveWorkDisposition(
      input({ prStatus: notFound('fresh', { unavailable_reason: 'command_failed' }) }),
    );
    expect(d.note?.kind).toBe('gh_unavailable');
    expect(d.showCleanUp).toBe(true);
  });

  it('no PR, refresh ok, clean work → clean_up primary, no warning note', () => {
    const d = deriveWorkDisposition(input({ prStatus: notFound('fresh'), workChange: cleanWorkChange() }));
    expect(d.primary).toBe('clean_up');
    expect(d.showCleanUp).toBe(true);
    expect(d.note).toBeNull();
  });

  it('no PR, dirty PR-ready work → create PR primary, cleanup hidden', () => {
    const createUrl = 'https://github.com/acme/repo/compare/main...task-1?expand=1';
    const d = deriveWorkDisposition(input({
      prStatus: notFound('fresh'),
      workChange: {
        kind: 'dirty_pr_ready',
        create_pr_url: createUrl,
        branch_name: 'task-1',
        base_branch: 'main',
      },
    }));
    expect(d.primary).toBe('resolve');
    expect(d.resolve).toEqual({ kind: 'create_pr', url: createUrl, branchName: 'task-1' });
    expect(d.showCleanUp).toBe(false);
    expect(d.showAbandon).toBe(true);
    expect(d.note?.kind).toBe('no_pr_dirty');
  });

  it('no PR, dirty uncommitted work → View Diff primary, cleanup hidden', () => {
    const d = deriveWorkDisposition(input({
      prStatus: notFound('fresh'),
      workChange: { kind: 'dirty_needs_review', reason: 'uncommitted_changes' },
    }));
    expect(d.primary).toBe('review');
    expect(d.resolve).toBeNull();
    expect(d.showCleanUp).toBe(false);
    expect(d.showAbandon).toBe(true);
    expect(d.note?.text).toContain('Uncommitted changes');
  });

  it('no PR, branch not pushed → View Diff primary and push/PR note', () => {
    const d = deriveWorkDisposition(input({
      prStatus: notFound('fresh'),
      workChange: { kind: 'dirty_needs_review', reason: 'branch_not_pushed' },
    }));
    expect(d.primary).toBe('review');
    expect(d.showCleanUp).toBe(false);
    expect(d.note?.text).toContain('push and open a PR');
  });

  it('no PR, work-change loading or unavailable → no cleanup hero', () => {
    for (const workChange of [loadingWorkChange(), { kind: 'unavailable' as const, reason: 'git failed' }]) {
      const d = deriveWorkDisposition(input({ prStatus: notFound('fresh'), workChange }));
      expect(d.primary).toBe('review');
      expect(d.showCleanUp).toBe(false);
      expect(d.showAbandon).toBe(true);
    }
  });

  it('no prStatus at all (not loading) → View Diff primary until work-change state is known', () => {
    const d = deriveWorkDisposition(input({ prStatus: null, workChange: null }));
    expect(d.primary).toBe('review');
    expect(d.showCleanUp).toBe(false);
  });
});

// --- Structural invariants ------------------------------------------------

describe('structural invariants', () => {
  // Enumerate a broad matrix of inputs and assert structural invariants on each.
  const displayStates: (PrDisplayState | undefined)[] = ['open', 'draft', 'merged', 'closed', undefined];
  const checkStates: (PrCheckState | undefined)[] = ['passing', 'pending', 'failing', 'unknown', undefined];
  const refreshStates: PrRefreshState[] = ['fresh', 'unavailable', 'not_found'];

  function* matrix(): Generator<WorkDispositionInput> {
    for (const convModeLabel of ['Work', 'Branch', 'Explore', undefined]) {
      for (const phaseType of ['idle', 'error', 'context_exhausted', 'awaiting_llm']) {
        for (const continuedInConvId of [null, 'conv-x']) {
          for (const prLoading of [false, true]) {
            for (const canSendMessage of [false, true]) {
              for (const found of [false, true]) {
                for (const display_state of displayStates) {
                  for (const check_state of checkStates) {
                    for (const rs of refreshStates) {
                      // null prStatus permutation (e.g. loading / disabled)
                      const base: WorkDispositionInput = {
                        convModeLabel,
                        phaseType,
                        continuedInConvId,
                        prStatus: null,
                        prLoading,
                        workChange: cleanWorkChange(),
                        canSendMessage,
                      };
                      yield base;

                      const pr: PrStatusResponse = found
                        ? {
                            found: true,
                            number: PR_NUMBER,
                            url: PR_URL,
                            ...(display_state ? { display_state } : {}),
                            ...(check_state ? { check_state } : {}),
                            refresh: refresh(rs),
                            work_change: cleanWorkChange(),
                          }
                        : { found: false, refresh: refresh(rs), work_change: cleanWorkChange() };
                      yield { ...base, prStatus: pr };
                    }
                  }
                }
              }
            }
          }
        }
      }
    }
  }

  it('always returns exactly one primary and never throws', () => {
    const valid: WorkDisposition['primary'][] = ['none', 'review', 'resolve', 'clean_up', 'abandon'];
    let count = 0;
    for (const inp of matrix()) {
      count++;
      const d = deriveWorkDisposition(inp);
      expect(valid).toContain(d.primary);
    }
    expect(count).toBeGreaterThan(1000);
  });

  it('resolve is non-null iff primary === resolve', () => {
    for (const inp of matrix()) {
      const d = deriveWorkDisposition(inp);
      expect(d.resolve !== null).toBe(d.primary === 'resolve');
    }
  });

  it('secondaryResolve, when present, rides only beside an address_feedback primary', () => {
    for (const inp of matrix()) {
      const d = deriveWorkDisposition(inp);
      if (d.secondaryResolve !== null) {
        expect(d.primary).toBe('resolve');
        expect(d.resolve?.kind).toBe('address_feedback');
        // It is always a GitHub link-out, never a second address button.
        expect(['merge_pr', 'open_pr']).toContain(d.secondaryResolve.kind);
      }
    }
  });

  it('hidden bars carry safe defaults', () => {
    for (const inp of matrix()) {
      const d = deriveWorkDisposition(inp);
      if (!d.visible) {
        expect(d.primary).toBe('none');
        expect(d.resolve).toBeNull();
        expect(d.showCleanUp).toBe(false);
        expect(d.note).toBeNull();
      }
    }
  });

  it('Abandon is hidden (on a visible bar) only in the continued case', () => {
    for (const inp of matrix()) {
      const d = deriveWorkDisposition(inp);
      if (d.visible && !d.showAbandon) {
        expect(d.note?.kind).toBe('continued');
        expect(d.primary).toBe('none');
      }
    }
  });

  it('gh_unavailable notes only appear with Clean up visible', () => {
    for (const inp of matrix()) {
      const d = deriveWorkDisposition(inp);
      if (d.note?.kind === 'gh_unavailable') {
        expect(d.showCleanUp).toBe(true);
        expect(d.primary).toBe('clean_up');
      }
    }
  });
});
