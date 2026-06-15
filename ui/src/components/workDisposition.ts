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
export type BarPrimary = 'none' | 'resolve' | 'clean_up' | 'abandon';

/**
 * The push-forward verb in the RESOLVE zone. External-link variants carry the
 * PR's GitHub url + number so the component renders an honest `<a>` (REQ-WAB-010);
 * Phoenix has no merge API and never opens a non-passing PR as "Merge".
 */
export type ResolveVerb =
  | { kind: 'address_feedback' }
  | { kind: 'merge_pr'; url: string; number: number }
  | { kind: 'open_pr'; url: string; number: number };

/** At most one inline note is shown per render. */
export type DispositionNote =
  | { kind: 'continued'; text: string }
  | { kind: 'checking'; text: string }
  | { kind: 'pr_closed'; text: string }
  | { kind: 'pr_open_stuck'; text: string }
  | { kind: 'gh_unavailable'; text: string };

export interface WorkDisposition {
  /** Bar renders only on Work/Branch conversations in a disposable phase. */
  visible: boolean;
  /** Exactly one glowing button across the whole bar (REQ-WAB-003). */
  primary: BarPrimary;
  /** Non-null iff `primary === 'resolve'`. */
  resolve: ResolveVerb | null;
  /** Render the Clean up (mark-merged) verb in the FINISH zone. */
  showCleanUp: boolean;
  /** gh unavailable: Clean up is a single-click manual fallback with a warning note. */
  cleanUpIsManualFallback: boolean;
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
    showCleanUp: false,
    cleanUpIsManualFallback: false,
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
  const { convModeLabel, phaseType, continuedInConvId, prStatus, prLoading, canSendMessage } = input;

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
      showCleanUp: false,
      cleanUpIsManualFallback: false,
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
      showCleanUp: false,
      cleanUpIsManualFallback: false,
      showAbandon: true,
      note: { kind: 'checking', text: NOTE_CHECKING },
    };
  }

  // Row 3. Stuck (error / context_exhausted): RESOLVE always suppressed
  // (REQ-WAB-005); primary collapses to a FINISH verb selected by PR state.
  if (stuck) {
    if (ds === 'merged') {
      return finish('clean_up', { showCleanUp: true });
    }
    if (ds === 'closed') {
      return finish('abandon', {
        note: {
          kind: 'pr_closed',
          text: `PR #${number} is closed without merge. Use Abandon to clean up.`,
        },
      });
    }
    if (ds === 'open' || ds === 'draft') {
      return finish('abandon', {
        note: {
          kind: 'pr_open_stuck',
          text: `PR #${number} still open — merge on GitHub, or abandon.`,
        },
      });
    }
    if (ghUnavailable) {
      return finish('clean_up', {
        showCleanUp: true,
        cleanUpIsManualFallback: true,
        note: { kind: 'gh_unavailable', text: NOTE_GH_UNAVAILABLE },
      });
    }
    // No PR, refresh ok.
    return finish('clean_up', { showCleanUp: true });
  }

  // From here: phaseType === 'idle'.

  // Row 4. idle, found, PR open/draft — push-forward RESOLVE.
  if (found && (ds === 'open' || ds === 'draft')) {
    // gh cannot confirm a stale persisted PR (refresh failed over a previously
    // observed PR). Keep the manual Clean up fallback (REQ-WL-003) rather than
    // only offering a PR link — the work may have merged externally and the
    // user must still be able to dispose without a working gh.
    if (prStatus?.refresh?.state === 'unavailable' && prStatus?.refresh?.stale) {
      return finish('clean_up', {
        showCleanUp: true,
        cleanUpIsManualFallback: true,
        note: { kind: 'gh_unavailable', text: NOTE_GH_UNAVAILABLE },
      });
    }
    // Addressable: an open PR with something to act on — failing checks, fresh
    // feedback, or a feedback coverage gap (e.g. an unreadable surface / auth
    // gap). The coverage marker only renders on the Address feedback button, so
    // a coverage-only gap must route here or its actionable hint is hidden.
    const addressable =
      ds === 'open' &&
      canSendMessage &&
      prStatus?.refresh?.state !== 'unavailable' &&
      (prStatus?.check_state === 'failing' ||
        prStatus?.feedback_freshness != null ||
        prStatus?.feedback_coverage != null);

    if (addressable) {
      return resolveVerb({ kind: 'address_feedback' });
    }

    if (ds === 'open' && prStatus?.check_state === 'passing') {
      // Honest "Merge" requires a real GitHub link. If url/number are somehow
      // missing on a found open passing PR (should not happen), fall through to
      // an honest open_pr link, else to a safe Abandon primary.
      if (url != null && number != null) {
        return resolveVerb({ kind: 'merge_pr', url, number });
      }
    }

    // Draft, or open-but-not-addressable-and-not-green → honest "Open PR".
    if (url != null && number != null) {
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

  // Row 7. idle, gh unavailable (no PR identity) → Clean up, manual fallback.
  if (ghUnavailable) {
    return finish('clean_up', {
      showCleanUp: true,
      cleanUpIsManualFallback: true,
      note: { kind: 'gh_unavailable', text: NOTE_GH_UNAVAILABLE },
    });
  }

  // Row 8. idle, no PR found (refresh ok / no PR) → Clean up.
  return finish('clean_up', { showCleanUp: true });
}

/** Build a RESOLVE-primary disposition; Abandon stays present as a secondary. */
function resolveVerb(verb: ResolveVerb): WorkDisposition {
  return {
    visible: true,
    primary: 'resolve',
    resolve: verb,
    showCleanUp: false,
    cleanUpIsManualFallback: false,
    showAbandon: true,
    note: null,
  };
}

/** Build a FINISH-primary disposition (clean_up or abandon). */
function finish(
  primary: 'clean_up' | 'abandon',
  opts: {
    showCleanUp?: boolean;
    cleanUpIsManualFallback?: boolean;
    note?: DispositionNote;
  },
): WorkDisposition {
  return {
    visible: true,
    primary,
    resolve: null,
    showCleanUp: opts.showCleanUp ?? false,
    cleanUpIsManualFallback: opts.cleanUpIsManualFallback ?? false,
    showAbandon: true,
    note: opts.note ?? null,
  };
}
