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
    ...extra,
  };
}

/** No PR found; refresh state controls gh-availability. */
function notFound(refreshState: PrRefreshState, extra: Partial<PrStatusResponse> = {}): PrStatusResponse {
  return {
    found: false,
    refresh: refresh(refreshState),
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
    expect(d.cleanUpIsManualFallback).toBe(false);
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

  it('gh unavailable → clean_up primary, manual fallback, gh_unavailable note', () => {
    const d = deriveWorkDisposition(
      input({ phaseType: 'error', prStatus: notFound('unavailable', { unavailable_reason: 'gh_missing' }) }),
    );
    expect(d.primary).toBe('clean_up');
    expect(d.showCleanUp).toBe(true);
    expect(d.cleanUpIsManualFallback).toBe(true);
    expect(d.note?.kind).toBe('gh_unavailable');
  });

  it('no PR, refresh ok → clean_up primary, no note', () => {
    const d = deriveWorkDisposition(input({ phaseType: 'context_exhausted', prStatus: notFound('fresh') }));
    expect(d.primary).toBe('clean_up');
    expect(d.showCleanUp).toBe(true);
    expect(d.cleanUpIsManualFallback).toBe(false);
    expect(d.note).toBeNull();
  });
});

// --- Row 4: idle open/draft RESOLVE ---------------------------------------

describe('idle open/draft — RESOLVE', () => {
  const fresh: PrFeedbackFreshness = { state: 'new', count: 2 };

  it('address_feedback when open + failing checks + canSendMessage', () => {
    const d = deriveWorkDisposition(
      input({ prStatus: foundPr('open', { check_state: 'failing' as PrCheckState }) }),
    );
    expect(d.primary).toBe('resolve');
    expect(d.resolve).toEqual({ kind: 'address_feedback' });
    expect(d.showCleanUp).toBe(false);
    expect(d.showAbandon).toBe(true);
  });

  it('coverage gap is orthogonal — passing checks + coverage-only routes to merge, NOT address (codex #C)', () => {
    const d = deriveWorkDisposition(
      input({
        prStatus: foundPr('open', {
          check_state: 'passing' as PrCheckState,
          feedback_coverage: { kind: 'auth_required', surfaces: ['review_threads'] },
        }),
      }),
    );
    // A coverage gap does NOT assert a feedback change, so it must not force
    // auto-fix routing / hide the Merge link. The coverage marker rides on the
    // resolve verb instead (rendered by the component).
    expect(d.primary).toBe('resolve');
    expect(d.resolve).toEqual({ kind: 'merge_pr', url: PR_URL, number: PR_NUMBER });
  });

  it('stale/unavailable refresh on a persisted open PR → Open PR link, no one-click cleanup (codex #E)', () => {
    const d = deriveWorkDisposition(
      input({
        prStatus: foundPr('open', {
          check_state: 'passing' as PrCheckState,
          refresh: { state: 'unavailable', last_attempted_at: '2026-01-01T00:00:00Z', stale: true },
          unavailable_reason: 'command_failed',
        }),
      }),
    );
    // Refresh failure must not make Clean up the primary (would delete a worktree
    // for a still-open PR). Route to Open PR (verify on GitHub); Abandon still disposes.
    expect(d.primary).toBe('resolve');
    expect(d.resolve).toEqual({ kind: 'open_pr', url: PR_URL, number: PR_NUMBER });
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

  it('NOT addressable when refresh unavailable on a found open PR → open_pr', () => {
    const d = deriveWorkDisposition(
      input({
        prStatus: foundPr('open', {
          check_state: 'failing' as PrCheckState,
          refresh: refresh('unavailable'),
        }),
      }),
    );
    expect(d.resolve).toEqual({ kind: 'open_pr', url: PR_URL, number: PR_NUMBER });
  });

  it('merge_pr when open + passing checks', () => {
    const d = deriveWorkDisposition(
      input({ prStatus: foundPr('open', { check_state: 'passing' as PrCheckState }) }),
    );
    expect(d.primary).toBe('resolve');
    expect(d.resolve).toEqual({ kind: 'merge_pr', url: PR_URL, number: PR_NUMBER });
  });

  it('open_pr when open + pending checks (not addressable, not green)', () => {
    const d = deriveWorkDisposition(
      input({ prStatus: foundPr('open', { check_state: 'pending' as PrCheckState }) }),
    );
    expect(d.resolve).toEqual({ kind: 'open_pr', url: PR_URL, number: PR_NUMBER });
  });

  it('open_pr (never merge) for a draft even with passing checks', () => {
    const d = deriveWorkDisposition(
      input({ prStatus: foundPr('draft', { check_state: 'passing' as PrCheckState }) }),
    );
    expect(d.resolve).toEqual({ kind: 'open_pr', url: PR_URL, number: PR_NUMBER });
  });

  it('open passing PR missing url/number → safe abandon fallback (should not happen)', () => {
    const pr = foundPr('open', { check_state: 'passing' as PrCheckState });
    delete pr.url;
    delete pr.number;
    const d = deriveWorkDisposition(input({ prStatus: pr }));
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

  it('gh unavailable → clean_up primary, manual fallback, gh_unavailable note', () => {
    const d = deriveWorkDisposition(
      input({ prStatus: notFound('unavailable', { unavailable_reason: 'not_authenticated' }) }),
    );
    expect(d.primary).toBe('clean_up');
    expect(d.cleanUpIsManualFallback).toBe(true);
    expect(d.note?.kind).toBe('gh_unavailable');
  });

  it('gh unavailable via unavailable_reason even when refresh state is fresh', () => {
    const d = deriveWorkDisposition(
      input({ prStatus: notFound('fresh', { unavailable_reason: 'command_failed' }) }),
    );
    expect(d.note?.kind).toBe('gh_unavailable');
    expect(d.cleanUpIsManualFallback).toBe(true);
  });

  it('no PR, refresh ok → clean_up primary, no manual fallback', () => {
    const d = deriveWorkDisposition(input({ prStatus: notFound('fresh') }));
    expect(d.primary).toBe('clean_up');
    expect(d.showCleanUp).toBe(true);
    expect(d.cleanUpIsManualFallback).toBe(false);
    expect(d.note).toBeNull();
  });

  it('no prStatus at all (not loading) → clean_up primary (no PR, refresh ok)', () => {
    const d = deriveWorkDisposition(input({ prStatus: null }));
    expect(d.primary).toBe('clean_up');
    expect(d.showCleanUp).toBe(true);
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
                          }
                        : { found: false, refresh: refresh(rs) };
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
    const valid: WorkDisposition['primary'][] = ['none', 'resolve', 'clean_up', 'abandon'];
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

  it('cleanUpIsManualFallback implies showCleanUp and a gh_unavailable note', () => {
    for (const inp of matrix()) {
      const d = deriveWorkDisposition(inp);
      if (d.cleanUpIsManualFallback) {
        expect(d.showCleanUp).toBe(true);
        expect(d.note?.kind).toBe('gh_unavailable');
      }
    }
  });
});
