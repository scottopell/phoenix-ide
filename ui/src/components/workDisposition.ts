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

type AddressFeedbackVerb = Extract<ResolveVerb, { kind: 'address_feedback' }>;
type PrLinkVerb = Extract<ResolveVerb, { kind: 'merge_pr' | 'open_pr' }>;
type PrimaryLinkVerb = Exclude<ResolveVerb, AddressFeedbackVerb>;

/** At most one inline note is shown per render. */
export type DispositionNote =
  | { kind: 'continued'; text: string }
  | { kind: 'checking'; text: string }
  | { kind: 'pr_closed'; text: string }
  | { kind: 'pr_open_stuck'; text: string }
  | { kind: 'no_pr_dirty'; text: string }
  | { kind: 'gh_unavailable'; text: string };

interface HiddenDisposition {
  visible: false;
  primary: 'none';
  resolve: null;
  secondaryResolve: null;
  showCleanUp: false;
  showAbandon: false;
  note: null;
}

interface ContinuedDisposition {
  visible: true;
  primary: 'none';
  resolve: null;
  secondaryResolve: null;
  showCleanUp: false;
  showAbandon: false;
  note: Extract<DispositionNote, { kind: 'continued' }>;
}

interface AddressFeedbackDisposition {
  visible: true;
  primary: 'resolve';
  resolve: AddressFeedbackVerb;
  secondaryResolve: PrLinkVerb | null;
  showCleanUp: false;
  showAbandon: true;
  note: null;
}

interface LinkResolveDisposition {
  visible: true;
  primary: 'resolve';
  resolve: PrimaryLinkVerb;
  secondaryResolve: null;
  showCleanUp: false;
  showAbandon: true;
  note: Extract<DispositionNote, { kind: 'no_pr_dirty' }> | null;
}

interface ReviewDisposition {
  visible: true;
  primary: 'review';
  resolve: null;
  secondaryResolve: null;
  showCleanUp: false;
  showAbandon: true;
  note: Extract<DispositionNote, { kind: 'no_pr_dirty' }>;
}

interface CleanUpDisposition {
  visible: true;
  primary: 'clean_up';
  resolve: null;
  secondaryResolve: null;
  showCleanUp: true;
  showAbandon: true;
  note: Extract<DispositionNote, { kind: 'gh_unavailable' }> | null;
}

interface AbandonDisposition {
  visible: true;
  primary: 'abandon';
  resolve: null;
  secondaryResolve: null;
  showCleanUp: boolean;
  showAbandon: true;
  note: Extract<DispositionNote, { kind: 'checking' | 'pr_closed' | 'pr_open_stuck' }> | null;
}

/**
 * Display state for the work-actions bar. Each variant carries only compatible
 * presentation fields, so hidden/resolve/continued states cannot contradict
 * their primary action or supporting verbs.
 */
export type WorkDisposition =
  | HiddenDisposition
  | ContinuedDisposition
  | AddressFeedbackDisposition
  | LinkResolveDisposition
  | ReviewDisposition
  | CleanUpDisposition
  | AbandonDisposition;

export interface WorkDispositionInput {
  convModeLabel: string | undefined;
  /** 'idle' | 'error' | other (other => bar hidden). */
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
function hidden(): HiddenDisposition {
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

const ELIGIBLE_PHASES = new Set(['idle', 'error']);
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
  const stuck = phaseType === 'error';

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

  // Row 3. Stuck error: RESOLVE always suppressed
  // (REQ-WAB-005); primary collapses to a FINISH verb selected by PR state.
  if (stuck) {
    // A stuck bar keeps BOTH terminal verbs visible (DispositionStuck): a stuck
    // conversation must be maximally disposable, so Clean up stays available
    // even when Abandon is the primary. Only the primary varies by PR state.
    if (ds === 'merged') {
      return cleanUp({});
    }
    if (ds === 'closed') {
      return abandon({
        showCleanUp: true,
        note: {
          kind: 'pr_closed',
          text: `PR #${number} is closed without merge. Use Abandon to clean up.`,
        },
      });
    }
    if (ds === 'open' || ds === 'draft') {
      return abandon({
        showCleanUp: true,
        note: {
          kind: 'pr_open_stuck',
          text: `PR #${number} still open — merge on GitHub, or abandon.`,
        },
      });
    }
    if (ghUnavailable) {
      return cleanUp({
        note: { kind: 'gh_unavailable', text: NOTE_GH_UNAVAILABLE },
      });
    }
    // No PR, refresh ok.
    return cleanUp({});
  }

  // From here: phaseType === 'idle'.

  // Row 4. idle, found, PR open/draft — push-forward RESOLVE. Cleanup is
  // suppressed here: an open PR is not done, so the primary moves the work
  // forward (address / merge / open on GitHub), never a one-click cleanup.
  if (found && (ds === 'open' || ds === 'draft')) {
    const refreshUnavailable = prStatus?.refresh?.state === 'unavailable';
    const hasLink = url != null && number != null;
    const passing = prStatus?.check_state === 'passing';

    // Addressable open PRs keep Address feedback primary even when refresh is
    // unavailable; refresh only changes the secondary link-out.
    const addressable = ds === 'open' && canSendMessage;

    if (addressable) {
      const secondary: PrLinkVerb | null =
        passing && !refreshUnavailable && hasLink ? { kind: 'merge_pr', url, number } :
          hasLink ? { kind: 'open_pr', url, number } : null;
      return addressFeedback(secondary);
    }

    // Non-addressable open PRs only get Merge when a fresh status confirms
    // passing checks; otherwise the link-out is labelled Open PR.
    if (ds === 'open' && !refreshUnavailable && passing && hasLink) {
      return linkResolve({ kind: 'merge_pr', url, number });
    }

    // Draft, stale/unavailable refresh, or open-but-not-green → honest
    // "Open PR" link.
    if (hasLink) {
      return linkResolve({ kind: 'open_pr', url, number });
    }
    // No usable url for a found open/draft PR — should not happen; Abandon is
    // the safe fallback rather than a broken link.
    return abandon({});
  }

  // Row 5. idle, found, merged → Clean up.
  if (found && ds === 'merged') {
    return cleanUp({});
  }

  // Row 6. idle, found, closed unmerged → Abandon.
  if (found && ds === 'closed') {
    return abandon({
      note: {
        kind: 'pr_closed',
        text: `PR #${number} is closed without merge. Use Abandon to clean up.`,
      },
    });
  }

  // Row 7. idle, gh unavailable (no PR identity) → Clean up with a warning note.
  if (ghUnavailable) {
    return cleanUp({
      note: { kind: 'gh_unavailable', text: NOTE_GH_UNAVAILABLE },
    });
  }

  // Row 8. idle, no PR found → split by work-change state.
  const noPrWorkChange = workChange ?? { kind: 'loading' as const };
  if (noPrWorkChange.kind === 'clean') {
    return cleanUp({});
  }
  if (noPrWorkChange.kind === 'dirty_pr_ready') {
    return linkResolve(
      {
        kind: 'create_pr',
        url: noPrWorkChange.create_pr_url,
        branchName: noPrWorkChange.branch_name,
      },
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

function addressFeedback(secondaryResolve: PrLinkVerb | null): AddressFeedbackDisposition {
  return {
    visible: true,
    primary: 'resolve',
    resolve: { kind: 'address_feedback' },
    secondaryResolve,
    showCleanUp: false,
    showAbandon: true,
    note: null,
  };
}

function linkResolve(
  resolve: PrimaryLinkVerb,
  note: LinkResolveDisposition['note'] = null,
): LinkResolveDisposition {
  return {
    visible: true,
    primary: 'resolve',
    resolve,
    secondaryResolve: null,
    showCleanUp: false,
    showAbandon: true,
    note,
  };
}

function reviewPrimary(text: string): ReviewDisposition {
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

function cleanUp(opts: { note?: CleanUpDisposition['note'] }): CleanUpDisposition {
  return {
    visible: true,
    primary: 'clean_up',
    resolve: null,
    secondaryResolve: null,
    showCleanUp: true,
    showAbandon: true,
    note: opts.note ?? null,
  };
}

function abandon(opts: {
  showCleanUp?: boolean;
  note?: AbandonDisposition['note'];
}): AbandonDisposition {
  return {
    visible: true,
    primary: 'abandon',
    resolve: null,
    secondaryResolve: null,
    showCleanUp: opts.showCleanUp ?? false,
    showAbandon: true,
    note: opts.note ?? null,
  };
}
