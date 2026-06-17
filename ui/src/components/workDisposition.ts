import type { PrStatusResponse } from '../api';

/**
 * Pure derivation of the work-actions bar's display state. This is the testable
 * heart of the bar: a total function from conversation + PR inputs to a single
 * `WorkDisposition`. No React, no DOM, no side effects, no clock or randomness.
 *
 * The single-primary rule (REQ-WAB-003) is encoded structurally: `primary` names
 * exactly one glowing slot across the whole bar (or `'none'`), so two glowing
 * buttons are unrepresentable rather than forbidden by a runtime check. The
 * RESOLVE verb is carried in `resolve`, non-null exactly when `primary` is
 * `'resolve'` (mirrors the Allium `ResolveVerbPresentIffVisible` invariant).
 *
 * Authoritative spec: specs/work-actions-bar/ (REQ-WAB-001..010 and
 * work-actions-bar.allium). The component imports this module and renders from
 * the returned value; this module never imports the component.
 */

/** The single glowing slot across the whole bar (REQ-WAB-003). */
export type BarPrimary = 'none' | 'review' | 'resolve' | 'clean_up' | 'abandon';

/**
 * The push-forward verb in the RESOLVE zone. External-link variants carry the
 * PR's GitHub url + number so the component renders an honest `<a>` (REQ-WAB-010);
 * Phoenix has no merge API and never opens a non-passing PR as "Merge".
 */
export type ResolveVerb =
  | { kind: 'address_feedback' }
  | { kind: 'merge_pr'; url: string; number: number }
  | { kind: 'open_pr'; url: string; number: number }
  | { kind: 'create_pr'; url: string; branchName: string };

/** At most one inline note is shown per render. */
export type DispositionNote =
  | { kind: 'continued'; text: string }
  | { kind: 'checking'; text: string }
  | { kind: 'pr_closed'; text: string }
  | { kind: 'pr_open_stuck'; text: string }
  | { kind: 'no_pr_dirty'; text: string }
  | { kind: 'gh_unavailable'; text: string };

export interface WorkDisposition {
  /** Bar renders only on Work/Branch conversations in a disposable phase. */
  visible: boolean;
  /** Exactly one glowing button across the whole bar (REQ-WAB-003). */
  primary: BarPrimary;
  /** Non-null iff `primary === 'resolve'`. */
  resolve: ResolveVerb | null;
  /**
   * A second, non-glowing RESOLVE verb shown beside the primary. Carries the
   * honest Merge/Open PR link-out when the primary is `address_feedback`, so an
   * open PR offers both "push the work forward" and "go to GitHub" at once
   * without a second glowing primary. Non-null implies `primary === 'resolve'`
   * (REQ-WAB-003); always null otherwise.
   */
  secondaryResolve: ResolveVerb | null;
  /** Render the Clean up (mark-merged) verb in the FINISH zone. */
  showCleanUp: boolean;
  /** Render the Abandon verb in the FINISH zone. False when continued (the
   *  continuation owns disposal — REQ-WAB-009 suppresses FINISH verbs, and the
   *  no-disabled-as-status rule means we show the note, not a dead button). */
  showAbandon: boolean;
  /** At most one inline note. */
  note: DispositionNote | null;
}

export interface WorkDispositionInput {
  convModeLabel: string | undefined;
  /** 'idle' | 'error' | 'context_exhausted' | other (other => bar hidden). */
  phaseType: string;
  continuedInConvId: string | null | undefined;
  /** Resolved PR status, or null while still loading / disabled. */
  prStatus: PrStatusResponse | null;
  /** PR status still loading (no usable prStatus yet). */
  prLoading: boolean;
  /** Worktree/branch change state, structurally supplied by the backend. */
  workChange: PrStatusResponse['work_change'] | null;
  /** onSendMessage available — false for stuck bars, gates Address feedback. */
  canSendMessage: boolean;
}

const NOTE_CONTINUED = 'Continued — actions belong on the continuation.';
const NOTE_CHECKING = 'Checking PR…';
const NOTE_GH_UNAVAILABLE = 'gh unavailable — manual cleanup.';

/** Safe defaults for a hidden bar — nothing glows, nothing renders. */
function hidden(): WorkDisposition {
  return {
    visible: false,
    primary: 'none',
    resolve: null,
    secondaryResolve: null,
    showCleanUp: false,
    showAbandon: false,
    note: null,
  };
}

const ELIGIBLE_PHASES = new Set(['idle', 'error', 'context_exhausted']);
const ELIGIBLE_MODES = new Set(['Work', 'Branch']);

/**
 * Derive the bar's display state. Total: every input combination returns a valid
 * `WorkDisposition` and this function never throws. First match wins, matching
 * the REQ-WAB-004 table order exactly.
 */
export function deriveWorkDisposition(input: WorkDispositionInput): WorkDisposition {
  const { convModeLabel, phaseType, continuedInConvId, prStatus, prLoading, workChange, canSendMessage } = input;

  // VISIBILITY (REQ-WAB-001): Work/Branch mode AND a disposable phase.
  if (!ELIGIBLE_MODES.has(convModeLabel ?? '') || !ELIGIBLE_PHASES.has(phaseType)) {
    return hidden();
  }

  const found = !!prStatus?.found;
  const ds = prStatus?.display_state;
  // gh unavailable: no PR identity AND the refresh probe (or unavailable_reason)
  // reports gh cannot confirm. Either signal counts.
  const ghUnavailable =
    !found && (prStatus?.refresh?.state === 'unavailable' || prStatus?.unavailable_reason != null);

  const number = prStatus?.number ?? prStatus?.pr?.number;
  const url = prStatus?.url ?? prStatus?.pr?.url;
  const stuck = phaseType === 'error' || phaseType === 'context_exhausted';

  // Row 1. Continued — no primary, all terminal verbs suppressed (REQ-WAB-009).
  if (continuedInConvId != null && continuedInConvId !== '') {
    return {
      visible: true,
      primary: 'none',
      resolve: null,
      secondaryResolve: null,
      showCleanUp: false,
      showAbandon: false,
      note: { kind: 'continued', text: NOTE_CONTINUED },
    };
  }

  // Row 2. Checking — PR status still loading, nothing usable yet. Abandon is
  // always safe; do NOT render a disabled Clean up (no-disabled-as-status).
  if (prLoading && !prStatus) {
    return {
      visible: true,
      primary: 'abandon',
      resolve: null,
      secondaryResolve: null,
      showCleanUp: false,
      showAbandon: true,
      note: { kind: 'checking', text: NOTE_CHECKING },
    };
  }

  // Row 3. Stuck (error / context_exhausted): RESOLVE always suppressed
  // (REQ-WAB-005); primary collapses to a FINISH verb selected by PR state.
  if (stuck) {
    // A stuck bar keeps BOTH terminal verbs visible (DispositionStuck): a stuck
    // conversation must be maximally disposable, so Clean up stays available
    // even when Abandon is the primary. Only the primary varies by PR state.
    if (ds === 'merged') {
      return finish('clean_up', { showCleanUp: true });
    }
    if (ds === 'closed') {
      return finish('abandon', {
        showCleanUp: true,
        note: {
          kind: 'pr_closed',
          text: `PR #${number} is closed without merge. Use Abandon to clean up.`,
        },
      });
    }
    if (ds === 'open' || ds === 'draft') {
      return finish('abandon', {
        showCleanUp: true,
        note: {
          kind: 'pr_open_stuck',
          text: `PR #${number} still open — merge on GitHub, or abandon.`,
        },
      });
    }
    if (ghUnavailable) {
      return finish('clean_up', {
        showCleanUp: true,
        note: { kind: 'gh_unavailable', text: NOTE_GH_UNAVAILABLE },
      });
    }
    // No PR, refresh ok.
    return finish('clean_up', { showCleanUp: true });
  }

  // From here: phaseType === 'idle'.

  // Row 4. idle, found, PR open/draft — push-forward RESOLVE. Cleanup is
  // suppressed here: an open PR is not done, so the primary moves the work
  // forward (address / merge / open on GitHub), never a one-click cleanup.
  if (found && (ds === 'open' || ds === 'draft')) {
    const refreshUnavailable = prStatus?.refresh?.state === 'unavailable';
    const hasLink = url != null && number != null;
    const passing = prStatus?.check_state === 'passing';

    // Addressable: any open PR Phoenix can post an auto-fix message to. Review
    // comments may need addressing whether or not checks fail and whether or
    // not the freshness baseline has been seeded, so Address feedback is the
    // primary on every reachable open PR — not gated on failing checks or a
    // pre-existing freshness signal. Freshness/coverage ride as markers on the
    // button. When checks are confirmed passing on a fresh status, the honest
    // Merge link rides alongside as a non-glowing secondary so the bar offers
    // both "address the feedback" and "go merge it" without a second primary.
    const addressable = ds === 'open' && canSendMessage && !refreshUnavailable;

    if (addressable) {
      const secondary: ResolveVerb | null =
        passing && hasLink ? { kind: 'merge_pr', url, number } : null;
      return resolveVerb({ kind: 'address_feedback' }, secondary);
    }

    // Not addressable (draft, no message channel, or unavailable refresh):
    // "Merge" only when checks are confirmed passing on a fresh status. A stale
    // or unavailable refresh cannot assert mergeability, so it routes to the
    // honest "Open PR" link (verify on GitHub) — never a one-click cleanup.
    if (ds === 'open' && !refreshUnavailable && passing && hasLink) {
      return resolveVerb({ kind: 'merge_pr', url, number });
    }

    // Draft, stale/unavailable refresh, or open-but-not-green → honest
    // "Open PR" link.
    if (hasLink) {
      return resolveVerb({ kind: 'open_pr', url, number });
    }
    // No usable url for a found open/draft PR — should not happen; Abandon is
    // the safe fallback rather than a broken link.
    return finish('abandon', {});
  }

  // Row 5. idle, found, merged → Clean up.
  if (found && ds === 'merged') {
    return finish('clean_up', { showCleanUp: true });
  }

  // Row 6. idle, found, closed unmerged → Abandon.
  if (found && ds === 'closed') {
    return finish('abandon', {
      note: {
        kind: 'pr_closed',
        text: `PR #${number} is closed without merge. Use Abandon to clean up.`,
      },
    });
  }

  // Row 7. idle, gh unavailable (no PR identity) → Clean up with a warning note.
  if (ghUnavailable) {
    return finish('clean_up', {
      showCleanUp: true,
      note: { kind: 'gh_unavailable', text: NOTE_GH_UNAVAILABLE },
    });
  }

  // Row 8. idle, no PR found → split by work-change state.
  const noPrWorkChange = workChange ?? { kind: 'loading' as const };
  if (noPrWorkChange.kind === 'clean') {
    return finish('clean_up', { showCleanUp: true });
  }
  if (noPrWorkChange.kind === 'dirty_pr_ready') {
    return resolveVerb(
      {
        kind: 'create_pr',
        url: noPrWorkChange.create_pr_url,
        branchName: noPrWorkChange.branch_name,
      },
      null,
      { kind: 'no_pr_dirty', text: 'Changes found but no PR. Open a PR on GitHub before cleanup.' },
    );
  }
  if (noPrWorkChange.kind === 'dirty_needs_review') {
    return reviewPrimary(noPrDirtyNote(noPrWorkChange.reason));
  }
  if (noPrWorkChange.kind === 'unavailable') {
    return reviewPrimary('Could not inspect work changes. Review the diff before cleanup.');
  }
  return reviewPrimary('Checking work changes. Review the diff before cleanup.');
}

function noPrDirtyNote(reason: Extract<PrStatusResponse['work_change'], { kind: 'dirty_needs_review' }>['reason']): string {
  switch (reason) {
    case 'uncommitted_changes':
      return 'Uncommitted changes found. Review, commit, and push before opening a PR.';
    case 'branch_not_pushed':
      return 'Branch is not pushed. Review the diff, then push and open a PR.';
    case 'local_ahead_of_remote':
      return 'Local commits are not pushed. Review the diff, then push and open a PR.';
    case 'remote_diverged':
      return 'Branch diverged from origin. Review the diff and reconcile before opening a PR.';
    case 'non_github_remote':
      return 'Changes found on a non-GitHub remote. Review the diff before cleanup.';
    case 'unknown_remote':
      return 'Remote state is unknown. Review the diff before cleanup.';
    case 'unknown':
      return 'Changes found but no PR. Review the diff before cleanup.';
  }
}

/**
 * Build a RESOLVE-primary disposition; Abandon stays present as a secondary.
 * `secondary` is the optional non-glowing RESOLVE link-out shown beside the
 * primary (only meaningful when `verb` is `address_feedback`).
 */
function resolveVerb(
  verb: ResolveVerb,
  secondary: ResolveVerb | null = null,
  note: DispositionNote | null = null,
): WorkDisposition {
  return {
    visible: true,
    primary: 'resolve',
    resolve: verb,
    secondaryResolve: secondary,
    showCleanUp: false,
    showAbandon: true,
    note,
  };
}

function reviewPrimary(text: string): WorkDisposition {
  return {
    visible: true,
    primary: 'review',
    resolve: null,
    secondaryResolve: null,
    showCleanUp: false,
    showAbandon: true,
    note: { kind: 'no_pr_dirty', text },
  };
}

/** Build a FINISH-primary disposition (clean_up or abandon). */
function finish(
  primary: 'clean_up' | 'abandon',
  opts: {
    showCleanUp?: boolean;
    note?: DispositionNote;
  },
): WorkDisposition {
  return {
    visible: true,
    primary,
    resolve: null,
    secondaryResolve: null,
    showCleanUp: opts.showCleanUp ?? false,
    showAbandon: true,
    note: opts.note ?? null,
  };
}
