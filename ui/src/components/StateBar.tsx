import { useState, useRef, useEffect, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { Link } from 'react-router-dom';
import { useLastSseEventAtRef } from '../conversation/useConversationAtom';
import { FolderTree } from 'lucide-react';
import { canChangeModelInState, type Conversation, type ConversationState, type ModelInfo, type PrStatusResponse } from '../api';
import type { ConversationPrStatusState } from '../hooks/useConversationPrStatus';
import type { ConnectionState } from '../hooks';
import { useIsMobile } from '../hooks';
import { getStateDescription, isAgentWorking } from '../utils';
import { ContextIndicator } from './ContextIndicator';

const CheckIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="20 6 9 17 4 12" />
  </svg>
);

/**
 * Continuation can only be triggered from the idle phase. The trigger is
 * bundled with the `idle` discriminant so a non-idle phase structurally
 * cannot carry one — there is exactly one source of truth (the phase the
 * caller derived this from), eliminating the old two-prop race where
 * `onTriggerContinuation` and a separate `convState.type === 'idle'` read
 * could disagree (task 04001). Consumers narrow on `.phase`; TypeScript
 * forbids reaching `.onTrigger` without it, so no ad-hoc guard is needed.
 *
 * `unavailable` covers every non-idle phase (working, terminal, error,
 * awaiting-approval, …) — the only relevant fact is that continuation
 * cannot be triggered, not that the agent is "busy".
 */
export type ContinuationState =
  | { phase: 'idle'; onTrigger: () => void }
  | { phase: 'unavailable' };

interface StateBarProps {
  conversation: Conversation | null;
  convState: ConversationState;
  connectionState: ConnectionState;
  connectionAttempt: number;
  nextRetryIn: number | null;
  contextWindowUsed: number;
  /** Model's maximum context window in tokens */
  modelContextWindow: number;
  /** Available models from the API (used to populate the model picker) */
  availableModels?: ModelInfo[];
  onRetryNow?: () => void;
  /** Continuation trigger, structurally bound to the idle phase. Absent or
   *  `{ phase: 'unavailable' }` means the trigger is unavailable. */
  continuation?: ContinuationState;
  /** Callback invoked when the user selects a different model for this conversation */
  onUpgradeModel?: (newModelId: string) => void;
  /** `Date.now()` timestamp when the current tool_executing phase began.
   *  Used to render a live elapsed-time counter ("running bash ... 4s").
   *  `null` or `undefined` when not in tool_executing.
   *  @deprecated Stage A: superseded by `phaseStateUpdatedAt` (server-
   *  authoritative; covers every working phase). Retained while the
   *  tool widget header continues to read it; the StateBar's elapsed
   *  counter now derives from `phaseStateUpdatedAt`. */
  toolExecutingStartedAt?: number | null;
  /** Server clock (unix ms) at which the conversation entered its
   *  current phase. Sourced from `Conversation.state_updated_at` and
   *  bumped on every `StateChange` SSE event. Used by the StateBar's
   *  elapsed counter for every working phase (REQ-WPV-001). `null`
   *  before the first Init/StateChange lands. */
  phaseStateUpdatedAt?: number | null;
  /** Ref to the client-clock (unix ms) of the most recent SSE event
   *  observed on this connection (including the typed `ping` keep-alive).
   *  Fed into the heartbeat watchdog: when the connection is `connected`
   *  AND the conversation is in a working phase AND
   *  `now() - current > 35000`, the StateBar surfaces a "no signal from
   *  server for Ns" degraded indicator (REQ-WPV-004).
   *
   *  Passed as a REF, not a value: the heartbeat bumps on every token and
   *  every `ping`, but the watchdog only samples it on its 1s interval.
   *  Reading from a ref keeps the bump out of the render path so the
   *  StateBar subtree does not re-render per event (the indicator clears
   *  on the next tick after signal resumes — within the watchdog's own 1s
   *  granularity). `<ConnectedStateBar>` supplies it via
   *  `useLastSseEventAtRef`; tests inject a plain `{ current }`. */
  lastSseEventAtRef?: { readonly current: number };
  /** Request id of the LLM request whose first byte has been observed
   *  on this turn, or `null` before the first `LlmFirstByte` event.
   *  When non-null AND the phase is `llm_requesting`, the StateBar
   *  switches from `awaiting LLM response Ns` (with counter) to `streaming` (no
   *  counter) per REQ-WPV-007 — the stream itself is the visible
   *  progress signal so an additional counter is redundant. */
  firstByteRequestId?: string | null;
  /** Per-turn retry context populated by the `sse_llm_attempt` reducer.
   *  When non-null AND the conversation is in a working phase, the
   *  StateBar appends "(retry K/N <reason>)" after the base reason and
   *  the elapsed counter — REQ-WPV-003 / REQ-LRV-001. Cleared on
   *  `agent_done` and on terminal `error`. */
  turnRetryContext?: {
    attempt: number;
    maxAttempts: number;
    reasonText: string;
  } | null;
  /** Mobile/tablet-only: opens the file browser overlay. When omitted (e.g. on
   *  desktop where `FileExplorerPanel` provides the same affordance), the
   *  button is not rendered. The explicit `| undefined` is required under
   *  `exactOptionalPropertyTypes: true`: ConversationPage's call site
   *  assigns `undefined` from a ternary, which the strict mode rejects
   *  without this annotation. */
  onOpenFiles?: (() => void) | undefined;
  prStatusState?: ConversationPrStatusState;
}

/** Format a context window size in tokens for compact display (e.g. 200k, 1M). */
function formatContextWindow(n: number): string {
  if (n >= 1_000_000) {
    const m = n / 1_000_000;
    return `${Number.isInteger(m) ? m : m.toFixed(1)}M`;
  }
  if (n >= 1000) {
    return `${Math.round(n / 1000)}k`;
  }
  return n.toString();
}

/** Abbreviate model ID: "claude-sonnet-4-6" -> "sonnet-4.6", "gpt-5.5" -> "gpt-5.5" */
function abbreviateModel(model: string): string {
  if (!model.startsWith('claude-')) return model;
  const inner = model.slice(7); // strip "claude-"
  const lastHyphen = inner.lastIndexOf('-');
  if (lastHyphen > 0 && /^\d+$/.test(inner.slice(lastHyphen + 1))) {
    return inner.slice(0, lastHyphen) + '.' + inner.slice(lastHyphen + 1);
  }
  return inner;
}

/** Extract project name from cwd, project_name field, or worktree path */
function getProjectName(conversation: Conversation): string | null {
  // Prefer explicit project_name from backend
  if (conversation.project_name) return conversation.project_name;

  // For non-work modes, extract from cwd
  const cwd = conversation.cwd;
  if (!cwd) return null;

  // Skip worktree UUIDs -- they're meaningless
  if (cwd.includes('.phoenix/worktrees/')) return null;

  const parts = cwd.replace(/\/$/, '').split('/');
  return parts[parts.length - 1] || null;
}

function prBadgeClass(pr: PrStatusResponse): string {
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

function prBadgeLabel(pr: PrStatusResponse): string {
  const n = pr.number ? `#${pr.number}` : 'PR';
  if (pr.display_state === 'merged') return `${n} merged`;
  if (pr.display_state === 'closed') return `${n} closed`;
  if (pr.display_state === 'draft') return `${n} draft`;
  if (pr.check_state === 'passing') return `${n} checks ✓`;
  if (pr.check_state === 'failing') return `${n} checks ✗`;
  if (pr.check_state === 'pending') return `${n} checks ...`;
  return n;
}

function prTooltip(pr: PrStatusResponse): string {
  const label = pr.number ? `PR #${pr.number}` : 'PR';
  const title = pr.title ? ` — ${pr.title}` : '';
  const state = pr.display_state ?? 'unknown';
  const checks = pr.check_state ?? 'unknown';
  return `${label}${title}\nState: ${state}\nChecks: ${checks}`;
}

function prCheckStatusText(pr: PrStatusResponse): string {
  switch (pr.check_state) {
    case 'passing': return 'Passed';
    case 'pending': return 'Pending';
    case 'failing': return 'Failed';
    default: return 'Unknown';
  }
}

function prSummaryText(pr: PrStatusResponse): string {
  const s = pr.check_summary;
  if (!s) return 'No check details available';
  return `${s.passing} pass · ${s.pending} pending · ${s.failing} fail · ${s.skipped} skip · ${s.unknown} unknown`;
}

function PrStatusPopover({ pr }: { pr: PrStatusResponse }) {

  const attentionNames = [
    ...(pr.check_summary?.failing_names ?? []),
    ...(pr.check_summary?.pending_names ?? []),
  ].slice(0, 5);
  return (
    <div className="pr-popover" role="dialog" aria-label="PR CI monitoring">
      {pr.refresh.stale && (
        <div className="pr-popover-muted">Stale PR data; {prRefreshStaleText(pr)}.</div>
      )}
      <div className="pr-popover-row">
        <span>CI</span>
        <strong>{prCheckStatusText(pr)}</strong>
      </div>
      <div className="pr-popover-muted">{prSummaryText(pr)}</div>
      {attentionNames.length > 0 && (
        <div className="pr-popover-list" title={attentionNames.join('\n')}>
          {attentionNames.join(' · ')}
        </div>
      )}
      {pr.feedback_summary && (
        <div className="pr-popover-muted">
          PR feedback: {pr.feedback_summary.unresolved} unresolved / {pr.feedback_summary.total} found
        </div>
      )}
      {pr.url && <a href={pr.url} target="_blank" rel="noreferrer">Open PR/checks ↗</a>}
    </div>
  );
}
function unavailablePrHint(reason: PrStatusResponse['unavailable_reason']): string | null {
  switch (reason) {
    case 'gh_missing': return 'gh missing';
    case 'not_authenticated': return 'gh auth';
    case 'not_git_repo': return 'no worktree';
    case 'command_failed': return 'PR status unavailable';
    default: return null;
  }
}

/** Format elapsed seconds as a compact duration string.
 *  < 60s  -> "4s"
 *  >= 60s -> "1m 4s" (seconds part omitted when 0: "2m")
 */
function formatElapsed(seconds: number): string {
  if (seconds < 60) return `${seconds}s`;
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return s > 0 ? `${m}m ${s}s` : `${m}m`;
}

/** Heartbeat watchdog threshold (REQ-WPV-004). 35 s is ~2.3x the 15 s
 *  server keep-alive interval, so a single missed keep-alive does not
 *  trigger the watchdog. Defined here (rather than imported from a
 *  shared config) because the StateBar is the sole consumer; if a
 *  second consumer materialises this should move to `config.ts`. */
const HEARTBEAT_WATCHDOG_SECONDS = 35;

export function StateBar({
  conversation,
  convState,
  connectionState,
  connectionAttempt,
  nextRetryIn,
  contextWindowUsed,
  modelContextWindow,
  availableModels,
  onRetryNow,
  continuation,
  onUpgradeModel,
  toolExecutingStartedAt: _deprecatedToolStartedAt,
  phaseStateUpdatedAt,
  lastSseEventAtRef,
  firstByteRequestId,
  turnRetryContext,
  onOpenFiles,
  prStatusState,
}: StateBarProps) {
  // `toolExecutingStartedAt` is kept on the prop type for the
  // tool-widget header (which still reads it from the atom). The
  // StateBar's own elapsed counter switched to `phaseStateUpdatedAt`
  // in Stage A — the destructured value is intentionally unused here.
  void _deprecatedToolStartedAt;
  const [pickerOpen, setPickerOpen] = useState(false);
  const [pickerShowAll, setPickerShowAll] = useState(false);
  const [prPopoverOpen, setPrPopoverOpen] = useState(false);
  // Mobile breakpoint mirrors the @media (max-width: 768px) block in index.css.
  const isMobile = useIsMobile();
  const [mobileExpanded, setMobileExpanded] = useState(false);
  // Collapse the mobile-expanded section when the viewport widens past
  // mobile — otherwise a user who expanded on phone, rotated to landscape,
  // would see a desktop bar with a stale "expanded" affordance.
  useEffect(() => {
    if (!isMobile) setMobileExpanded(false);
  }, [isMobile]);
  const pickerRef = useRef<HTMLSpanElement>(null);
  const prRef = useRef<HTMLSpanElement>(null);

  // Live elapsed-time counter, generalized for every working phase
  // (REQ-WPV-001 / REQ-WPV-003). The source of truth is the server-
  // authoritative `phaseStateUpdatedAt` (unix ms) carried on
  // StateChange + Init, so the counter survives reconnect, page reload,
  // and multi-tab observation. Ticks every second; reset to 0 the
  // instant the phase leaves the working set.
  // Working-phase set (elapsed counter, heartbeat watchdog gating,
  // last-known-activity capture) is the single `isAgentWorking`
  // predicate from utils — same set the rest of the UI gates on, with
  // exhaustiveness enforced via `satisfies never`.
  const phaseIsWorking = isAgentWorking(convState);
  const [phaseElapsedSeconds, setPhaseElapsedSeconds] = useState(0);
  useEffect(() => {
    if (!phaseIsWorking || phaseStateUpdatedAt == null) {
      setPhaseElapsedSeconds(0);
      return;
    }
    // Compute immediately to avoid the 1s render lag after a phase
    // transition; then tick once per second.
    setPhaseElapsedSeconds(Math.max(0, Math.floor((Date.now() - phaseStateUpdatedAt) / 1000)));
    const interval = window.setInterval(() => {
      setPhaseElapsedSeconds(
        Math.max(0, Math.floor((Date.now() - phaseStateUpdatedAt) / 1000))
      );
    }, 1000);
    return () => window.clearInterval(interval);
  }, [phaseIsWorking, phaseStateUpdatedAt]);

  // Heartbeat watchdog (REQ-WPV-004). When the connection is healthy
  // AND the agent is working AND no SSE event of any kind (typed
  // `ping` keep-alives included) has arrived for HEARTBEAT_WATCHDOG_MS,
  // we surface a degraded-signal indicator. Ticks once per second to
  // re-evaluate; cleared the instant any event lands or the connection
  // leaves the healthy state. The 35 s threshold is ~2.3x the 15 s
  // server keep-alive interval — one missed keep-alive does not
  // trip the watchdog.
  const [watchdogSeconds, setWatchdogSeconds] = useState(0);
  const watchdogArmed =
    (connectionState === 'connected' || connectionState === 'reconnected') &&
    phaseIsWorking &&
    typeof lastSseEventAtRef?.current === 'number';
  useEffect(() => {
    if (!watchdogArmed) {
      setWatchdogSeconds(0);
      return;
    }
    // Read the heartbeat clock from the ref on each tick rather than from a
    // render-time value. The ref is mutated (silently, no re-render) by
    // `useLastSseEventAtRef` on every event; sampling it here at 1 Hz is the
    // watchdog's only consumer, so the StateBar never re-renders per event.
    // The interval is created once per armed window — `lastSseEventAtRef` is
    // a stable object, so a fresh event does not tear it down and rebuild it.
    const compute = () => {
      const elapsed = Math.max(0, Math.floor((Date.now() - lastSseEventAtRef!.current) / 1000));
      setWatchdogSeconds(elapsed);
    };
    compute();
    const interval = window.setInterval(compute, 1000);
    return () => window.clearInterval(interval);
  }, [watchdogArmed, lastSseEventAtRef]);
  const watchdogStale = watchdogArmed && watchdogSeconds >= HEARTBEAT_WATCHDOG_SECONDS;

  // Last-known activity capture (REQ-WPV-005). When the connection
  // leaves the healthy set during a working phase, freeze a snapshot
  // of (phase, elapsed-at-disconnect) so the reconnecting/offline
  // display shows "reconnecting (N) — last: awaiting LLM response 12s" instead of
  // masking the agent's state entirely. Cleared on return to a
  // healthy connection.
  const lastKnownActivityRef = useRef<{
    phase: ConversationState;
    elapsedSecondsAtDisconnect: number;
    // REQ-WPV-007: whether the live label was `streaming` (first byte had
    // landed) at the moment we degraded. Without this the frozen "last: …"
    // line regresses to "awaiting LLM response Ns" even though tokens were
    // already flowing when the connection dropped.
    wasStreaming: boolean;
  } | null>(null);
  const connectionHealthy =
    connectionState === 'connected' || connectionState === 'reconnected';
  useEffect(() => {
    if (connectionHealthy) {
      // Healthy connection — drop any frozen snapshot so the live
      // path takes over again.
      lastKnownActivityRef.current = null;
      return;
    }
    // Degraded — capture iff we were working at the moment we
    // degraded and we don't already have a snapshot for this
    // degraded window.
    if (
      lastKnownActivityRef.current == null &&
      phaseIsWorking &&
      phaseStateUpdatedAt != null
    ) {
      const wasStreaming =
        (convState.type === 'llm_requesting' ||
          convState.type === 'seeded_llm_requesting' ||
          convState.type === 'awaiting_llm') &&
        firstByteRequestId != null;
      lastKnownActivityRef.current = {
        phase: convState,
        elapsedSecondsAtDisconnect: Math.max(
          0,
          Math.floor((Date.now() - phaseStateUpdatedAt) / 1000)
        ),
        wasStreaming,
      };
    }
    // No-op on subsequent renders during the same degraded window —
    // the snapshot is by-construction frozen.
  }, [connectionHealthy, phaseIsWorking, phaseStateUpdatedAt, convState, firstByteRequestId]);

  // Close model picker on outside click
  useEffect(() => {
    if (!pickerOpen) return;
    const handleClick = (e: MouseEvent) => {
      if (pickerRef.current && !pickerRef.current.contains(e.target as Node)) {
        setPickerOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [pickerOpen]);

  // Close model picker on Escape
  useEffect(() => {
    if (!pickerOpen) return;
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setPickerOpen(false);
    };
    document.addEventListener('keydown', handleKey);
    return () => document.removeEventListener('keydown', handleKey);
  }, [pickerOpen]);

  useEffect(() => {
    if (!prPopoverOpen) return;
    const handleClick = (e: MouseEvent) => {
      if (!prRef.current || !prRef.current.contains(e.target as Node)) {
        setPrPopoverOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [prPopoverOpen]);

  useEffect(() => {
    if (!prPopoverOpen) return;
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setPrPopoverOpen(false);
    };
    document.addEventListener('keydown', handleKey);
    return () => document.removeEventListener('keydown', handleKey);
  }, [prPopoverOpen]);

  let dotClass = 'dot';
  let stateText = '';

  // REQ-WPV-003 / REQ-LRV-001: when a retry has fired this turn, append
  // "(retry K/N after <reason>)" to the base reason. Returns "" when no
  // retry context exists. Leading space so the suffix concatenates
  // cleanly onto either the live or the frozen-last-known reason.
  const retrySuffix =
    turnRetryContext != null
      ? ` (retry ${turnRetryContext.attempt}/${turnRetryContext.maxAttempts} after ${turnRetryContext.reasonText})`
      : '';

  // Format the working-phase reason as "<base> Ns <retry?>" (e.g.
  // "awaiting LLM response 4s (retry 2/3 after rate limit)",
  // "running bash 12s") for use in both the live and the frozen-last-
  // known-activity paths below.
  const formatWorkingReason = (
    phase: ConversationState,
    elapsedSeconds: number,
    streaming = false
  ): string => {
    // REQ-WPV-007: mirror the live path — if the first byte had landed,
    // the label is `streaming` (no elapsed counter), with the retry suffix
    // carried through. Used for the frozen last-known-activity display so a
    // mid-stream disconnect doesn't regress to "awaiting LLM response Ns".
    if (streaming) {
      return `streaming${retrySuffix}`;
    }
    // Strip a trailing `...` from the base label: descriptions for
    // working phases (`llm_requesting` → "awaiting LLM response...")
    // already end in an ellipsis. Appending `... <elapsed>` directly
    // would produce "awaiting LLM response... ... 7s".
    const base = getStateDescription(phase).replace(/\.{3}$/, '');
    const withElapsed =
      elapsedSeconds > 0 ? `${base} ... ${formatElapsed(elapsedSeconds)}` : base;
    return `${withElapsed}${retrySuffix}`;
  };

  if (!conversation) {
    dotClass += ' hidden';
    stateText = '';
  } else if (watchdogStale) {
    // REQ-WPV-004: degraded-signal indicator overrides every working-
    // state message. The user needs to know the channel is suspect
    // before they trust further detail.
    dotClass += ' degraded';
    stateText = `no signal from server for ${formatElapsed(watchdogSeconds)}`;
  } else {
    // Determine dot and text based on connection state first.
    switch (connectionState) {
      case 'disconnected':
        dotClass += ' connecting';
        stateText = 'connecting...';
        break;

      case 'connecting':
        dotClass += ' connecting';
        stateText = 'connecting...';
        break;

      case 'reconnecting': {
        // REQ-WPV-005: don't mask agent activity. If we were working
        // when we degraded, show both the connection chip AND the
        // last-known agent activity with its elapsed FROZEN at
        // disconnect (honest about what we don't know — the agent
        // may or may not still be doing the thing).
        dotClass += ' reconnecting';
        const snap = lastKnownActivityRef.current;
        if (snap) {
          stateText = `reconnecting (${connectionAttempt}) — last: ${formatWorkingReason(
            snap.phase,
            snap.elapsedSecondsAtDisconnect,
            snap.wasStreaming
          )}`;
        } else {
          stateText = `reconnecting (${connectionAttempt})...`;
        }
        break;
      }

      case 'offline': {
        // Same composition as reconnecting; the connection-chip text
        // changes from "reconnecting (N)" to "offline" but the
        // last-known activity is still surfaced when we have it.
        dotClass += ' offline';
        const snap = lastKnownActivityRef.current;
        if (snap) {
          stateText = `offline — last: ${formatWorkingReason(
            snap.phase,
            snap.elapsedSecondsAtDisconnect,
            snap.wasStreaming
          )}`;
        } else {
          stateText = 'offline';
        }
        break;
      }

      case 'reconnected':
        dotClass += ' reconnected';
        stateText = 'reconnected';
        break;

      case 'connected': {
        // When connected, show agent state
        switch (convState.type) {
          case 'idle':
            dotClass += ' idle';
            stateText = 'ready';
            break;
          case 'terminal':
          case 'handed_off':
            dotClass += ' terminal';
            stateText = convState.type === 'handed_off' ? 'handed off' : 'completed';
            break;
          case 'awaiting_task_approval':
            dotClass += ' approval';
            stateText = 'awaiting approval';
            break;
          case 'awaiting_user_response':
            dotClass += ' approval';
            // "your reply" disambiguates from `awaiting LLM response`
            // (the llm_requesting label). Both surface in the same
            // StateBar slot; the prose has to make clear who the user
            // is waiting on.
            stateText = 'awaiting your reply';
            break;
          case 'error':
            dotClass += ' error';
            stateText = 'error';
            break;
          case 'context_exhausted':
            dotClass += ' error';
            stateText = 'context full';
            break;
          case 'awaiting_llm': case 'llm_requesting': case 'seeded_llm_requesting': case 'tool_executing':
          case 'awaiting_sub_agents': case 'awaiting_continuation':
          case 'cancelling': case 'cancelling_tool': case 'cancelling_sub_agents':
          case 'awaiting_recovery':
            // REQ-WPV-001/003: elapsed counter is keyed off the
            // server-authoritative `phaseStateUpdatedAt`, so every
            // working phase gets a live "<reason> Ns" display (not
            // just tool_executing). The previous tool-only counter
            // is retained on the tool widget header itself via
            // `toolExecutingStartedAt`.
            dotClass += ' working';
            // REQ-WPV-007: once the first byte for the current LLM
            // request lands, switch the base reason from `awaiting LLM response Ns`
            // to `streaming` (no counter — the stream itself is the
            // progress signal). The transition applies only while
            // the phase is one of the llm_requesting family; tool/
            // sub-agent phases retain their elapsed counter.
            if (
              (convState.type === 'llm_requesting' ||
                convState.type === 'seeded_llm_requesting' ||
                convState.type === 'awaiting_llm') &&
              firstByteRequestId != null
            ) {
              // REQ-WPV-003: retry suffix carries through every working
              // phase, including streaming. A turn that retried once and
              // is now streaming should still surface "(retry 2/3 …)"
              // so the user has the full context for "why has this
              // taken so long?".
              stateText = `streaming${retrySuffix}`;
            } else {
              stateText = formatWorkingReason(convState, phaseElapsedSeconds);
            }
            break;
          default: convState satisfies never;
        }
        break;
      }

      default:
        dotClass += ' connecting';
        stateText = 'connecting...';
    }
  }

  const showOfflineBanner = connectionState === 'offline' && nextRetryIn !== null;

  // Context window indicator -- use model-specific limit, fallback for legacy
  const maxTokens = modelContextWindow || 200_000;
  // Trigger menu only available in idle phase (the indicator gates on the
  // context threshold itself). The capability is structurally inseparable
  // from the idle discriminant, so no `convState.type` re-check is needed
  // here — narrowing the discriminated union is the guard (task 04001).
  const indicatorTrigger =
    continuation?.phase === 'idle' ? continuation.onTrigger : undefined;

  // Derived display values
  const mode = conversation?.conv_mode_label?.toLowerCase();
  const isWork = mode === 'work';
  const isExplore = mode === 'explore';
  const isBranchMode = mode === 'branch';
  const modeLabel = conversation?.conv_mode_label;
  const modeSuffix = isExplore ? ' (read-only)' : '';
  const modeClass = `statebar-mode statebar-mode--${mode}`;
  const modelAbbrev = conversation ? abbreviateModel(conversation.model ?? '') : '';
  const projectName = conversation ? getProjectName(conversation) : null;

  // Model picker: available when no operation is in flight (idle or error)
  // and we have models and a callback. Error-state switch lets the user
  // recover from overload/quota by picking another model, then retrying.
  const currentModel = conversation?.model ?? '';
  const canPickModel = !!(
    onUpgradeModel &&
    availableModels &&
    availableModels.length > 0 &&
    canChangeModelInState(convState)
  );

  // Default list: recommended models plus the currently selected one (if not recommended).
  // "Show all" expands to the full list. Always deduplicate by id.
  const pickerModels: ModelInfo[] = (() => {
    if (!availableModels) return [];
    if (pickerShowAll) return availableModels;
    const recommended = availableModels.filter(m => m.recommended);
    if (currentModel && !recommended.some(m => m.id === currentModel)) {
      const current = availableModels.find(m => m.id === currentModel);
      if (current) return [current, ...recommended];
    }
    return recommended;
  })();

  const handleModelTriggerClick = () => {
    if (!canPickModel) return;
    setPickerOpen(v => !v);
  };

  const handleSelectModel = (modelId: string) => {
    setPickerOpen(false);
    if (!onUpgradeModel) return;
    if (modelId === currentModel) return;
    onUpgradeModel(modelId);
  };

  const baseBranch = conversation?.base_branch;
  const branchName = conversation?.branch_name;
  const taskTitle = conversation?.task_title;
  const prStatus = prStatusState?.status === 'ready' ? prStatusState.prStatus : null;
  const prLoading = prStatusState?.status === 'loading';
  const prHint = prStatus && !prStatus.found ? unavailablePrHint(prStatus.unavailable_reason) : null;

  const showMobileCollapsed = isMobile && !mobileExpanded;
  const headerProps = showMobileCollapsed
    ? {
        className: 'statebar-mobile-collapsed',
        role: 'button',
        tabIndex: 0,
        'aria-expanded': false,
        'aria-label': 'Expand status bar',
        onClick: () => setMobileExpanded(true),
        onKeyDown: (e: ReactKeyboardEvent) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            setMobileExpanded(true);
          }
        },
      }
    : {};

  return (
    <>
      <header id="state-bar" {...headerProps}>
        <div id="state-bar-left">
          {conversation ? (
            <>
              {/* Line 1: nav slug + mode + model */}
              <div className="statebar-line1">
                <Link to="/" className="statebar-slug" title="Back to conversations">
                  <span className="back-arrow">&larr;</span>
                  <span className="slug-text">{conversation.slug}</span>
                </Link>
                {modeLabel && (
                  <span className={modeClass} title={
                    isExplore ? 'Read-only mode (git project)' :
                    isWork ? 'Write mode (task branch)' :
                    isBranchMode ? 'Branch mode (existing branch)' :
                    'Full access (no git workflow)'
                  }>
                    {modeLabel}{modeSuffix}
                  </span>
                )}
                <span className="conv-model-wrapper" ref={pickerRef}>
                  {canPickModel ? (
                    <button
                      className="conv-model conv-model--button"
                      title={`Model: ${conversation.model ?? 'default'} (click to change)`}
                      onClick={handleModelTriggerClick}
                      aria-haspopup="listbox"
                      aria-expanded={pickerOpen}
                    >
                      {modelAbbrev}
                      <span className="conv-model-caret" aria-hidden="true">&#9662;</span>
                    </button>
                  ) : (
                    <span className="conv-model" title={`Model: ${conversation.model ?? 'default'}`}>
                      {modelAbbrev}
                    </span>
                  )}
                  {pickerOpen && canPickModel && (
                    <div className="model-picker" role="listbox" aria-label="Select model">
                      <div className="model-picker-list">
                        {pickerModels.map(m => {
                          const selected = m.id === currentModel;
                          return (
                            <button
                              key={m.id}
                              type="button"
                              role="option"
                              aria-selected={selected}
                              className={
                                'model-picker-item' +
                                (selected ? ' model-picker-item--selected' : '')
                              }
                              onClick={() => handleSelectModel(m.id)}
                              title={m.description || m.id}
                            >
                              <span className="model-picker-item-check" aria-hidden="true">
                                {selected ? <CheckIcon /> : null}
                              </span>
                              <span className="model-picker-item-id">{m.id}</span>
                              <span className="model-picker-item-ctx">
                                {formatContextWindow(m.context_window)}
                              </span>
                            </button>
                          );
                        })}
                      </div>
                      <label className="model-picker-show-all-toggle">
                        <input
                          type="checkbox"
                          checked={pickerShowAll}
                          onChange={(e) => setPickerShowAll(e.target.checked)}
                        />
                        <span>Show all models</span>
                      </label>
                    </div>
                  )}
                </span>
              </div>

              {/* Line 2: task title (Work) + git info, or project name */}
              {(taskTitle || branchName || projectName) && (
                <div className="statebar-line2">
                  {taskTitle && (
                    <span className="statebar-task-title" title={branchName ? `Branch: ${branchName}` : undefined}>
                      {taskTitle}
                    </span>
                  )}
                  {branchName && baseBranch && (
                    <span className={`git-flow${taskTitle ? ' git-flow--secondary' : ''}`} title={`${baseBranch} <- ${branchName}`}>
                      <span className="git-base">{baseBranch}</span>
                      <span className="git-arrow">&larr;</span>
                      <span className="git-branch">{branchName}</span>
                      {prStatus?.found && prStatus.url && (
                        <span className="pr-control" ref={prRef}>
                          <button
                            type="button"
                            className={prBadgeClass(prStatus)}
                            title={prTooltip(prStatus)}
                            aria-haspopup="dialog"
                            aria-expanded={prPopoverOpen}
                            onClick={() => setPrPopoverOpen(v => !v)}
                          >
                            {prBadgeLabel(prStatus)}
                          </button>
                          {prPopoverOpen && conversation && (
                            <PrStatusPopover pr={prStatus} />
                          )}
                        </span>
                      )}
                      {prHint && !prLoading && (
                        <span className="pr-hint" title="Install and authenticate GitHub CLI to enable PR tracking">
                          {prHint}
                        </span>
                      )}
                    </span>
                  )}
                  {branchName && !baseBranch && (
                    <span className="git-branch-solo" title={`Branch: ${branchName}`}>
                      {branchName}
                      {prStatus?.found && prStatus.url && (
                        <span className="pr-control" ref={prRef}>
                          <button
                            type="button"
                            className={prBadgeClass(prStatus)}
                            title={prTooltip(prStatus)}
                            aria-haspopup="dialog"
                            aria-expanded={prPopoverOpen}
                            onClick={() => setPrPopoverOpen(v => !v)}
                          >
                            {prBadgeLabel(prStatus)}
                          </button>
                          {prPopoverOpen && conversation && (
                            <PrStatusPopover pr={prStatus} />
                          )}
                        </span>
                      )}
                      {prHint && !prLoading && (
                        <span className="pr-hint" title="Install and authenticate GitHub CLI to enable PR tracking">
                          {prHint}
                        </span>
                      )}
                    </span>
                  )}
                  {projectName && (
                    <span className="statebar-project" title={conversation.cwd}>
                      {projectName}
                    </span>
                  )}
                </div>
              )}
            </>
          ) : (
            <span className="statebar-slug">&mdash;</span>
          )}
        </div>
        <div id="state-bar-right">
          <div id="state-indicator">
            <span id="state-dot" className={dotClass}></span>
            <span id="state-text">{stateText}</span>
          </div>
          {conversation && contextWindowUsed > 0 && (
            <ContextIndicator
              used={contextWindowUsed}
              max={maxTokens}
              conversationId={conversation.id}
              onTriggerContinuation={indicatorTrigger}
            />
          )}
          {onOpenFiles && (
            <button
              type="button"
              className="statebar-files-btn"
              onClick={(e) => {
                e.stopPropagation();
                onOpenFiles();
              }}
              onKeyDown={(e) => {
                // The collapsed StateBar (≤768px) has its own Enter/Space
                // handler on `<header>` that calls preventDefault to expand
                // the bar. Stop propagation so the button's default
                // activation isn't suppressed and the bar doesn't toggle.
                if (e.key === 'Enter' || e.key === ' ') {
                  e.stopPropagation();
                }
              }}
              aria-label="Browse project files"
              title="Browse project files"
            >
              <FolderTree size={18} aria-hidden="true" />
            </button>
          )}
        </div>
        {isMobile && (
          <button
            type="button"
            className="statebar-chevron"
            onClick={(e) => {
              e.stopPropagation();
              setMobileExpanded(v => !v);
            }}
            aria-label={mobileExpanded ? 'Collapse status bar' : 'Expand status bar'}
            aria-expanded={mobileExpanded}
          >
            {mobileExpanded ? '▾' : '▴'}
          </button>
        )}
      </header>
      {showOfflineBanner && (
        <div className="offline-banner">
          <span className="offline-banner-text">
            Connection lost. Reconnecting in {nextRetryIn}s...
          </span>
          {onRetryNow && (
            <button className="offline-banner-retry" onClick={onRetryNow}>
              Retry now
            </button>
          )}
        </div>
      )}
    </>
  );
}

/**
 * StateBar wired to the live heartbeat clock. Subscribes to `lastSseEventAt`
 * as a REF (`useLastSseEventAtRef`) rather than a value, so the per-event
 * bump (every token, every `ping`) does NOT re-render this wrapper — and
 * therefore does not re-render the StateBar subtree below it. The watchdog
 * reads the ref on its 1s interval. The page passes every other
 * (low-frequency) prop through; `<StateBar>` stays a pure presentational
 * component so its tests can inject a plain `{ current }` ref directly.
 */
export function ConnectedStateBar({
  slug,
  ...rest
}: Omit<StateBarProps, 'lastSseEventAtRef'> & { slug: string }) {
  const lastSseEventAtRef = useLastSseEventAtRef(slug);
  return <StateBar {...rest} lastSseEventAtRef={lastSseEventAtRef} />;
}
