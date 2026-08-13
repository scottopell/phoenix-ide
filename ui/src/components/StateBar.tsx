import {
  useState,
  useRef,
  useEffect,
  useCallback,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type RefObject,
} from "react";
import { Link } from "react-router-dom";
import { useLastSseEventAtRef } from "../conversation/useConversationAtom";
import { FolderTree, TerminalSquare } from "lucide-react";
import type { TerminalPanelStatus } from "./TerminalPanel";
import {
  canChangeModelInState,
  isTerminalConversationState,
  type Conversation,
  type ConversationState,
  type EffortCapabilities,
  type ModelEffort,
  type ModelInfo,
  type PrStatusResponse,
  type ServiceTier,
} from "../api";
import type { ConversationPrStatusHandle } from "../hooks/useConversationPrStatus";
import type { ConnectionState } from "../hooks";
import { useIsCompactLayout } from "../hooks";
import { getStateDescription, isAgentWorking } from "../utils";
import {
  getConversationIdentity,
} from '../utils/conversationIdentity';
import { ContextIndicator } from "./ContextIndicator";
import { SelectionDialog } from "./SelectionDialog";
import "./StateBar.css";
import { setActivePrSelectorIntent, type ActivePrSelectorIntent } from './activePrSelectorIntent';
import { derivePrRailAvailability } from './prRailAvailability';
import {
  prBadgeClass,
  prBadgeLabel,
  prTooltip,
  unavailablePrHint,
} from "./prBadge";

const CheckIcon = () => (
  <svg
    width="12"
    height="12"
    viewBox="0 0 24 24"
    fill="none"
    stroke="currentColor"
    strokeWidth="3"
    strokeLinecap="round"
    strokeLinejoin="round"
    aria-hidden="true"
  >
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
  | { phase: "idle"; onTrigger: () => void }
  | { phase: "unavailable" };

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
  /** Callback invoked when the user confirms model, effort, and Codex service tier together. */
  onUpgradeModel?: (
    newModelId: string,
    effort?: ModelEffort | null,
    serviceTier?: ServiceTier,
  ) => void | Promise<void>;
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
  /** Mobile/tablet-only terminal launcher rendered in expanded details. */
  terminalLauncher?: {
    status: TerminalPanelStatus;
    onOpen: () => void;
    buttonRef: RefObject<HTMLButtonElement>;
  } | undefined;
  workActionsAvailable?: boolean;
  prStatusHandle?: ConversationPrStatusHandle;
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

/** Abbreviate model ID: "claude-sonnet-4-6" -> "sonnet-4.6", "claude-sonnet-5" -> "sonnet-5". */
function abbreviateModel(model: string): string {
  if (!model.startsWith("claude-")) return model;
  const inner = model.slice(7); // strip "claude-"
  const versionMatch = inner.match(/^(.*-\d+)-(\d+)$/);
  if (versionMatch) return `${versionMatch[1]}.${versionMatch[2]}`;
  return inner;
}

const EFFORT_LABELS: Record<ModelEffort, string> = {
  none: 'None',
  minimal: 'Minimal',
  low: 'Low',
  medium: 'Medium',
  high: 'High',
  xhigh: 'X-High',
  max: 'Max',
};

function effortLabel(effort: ModelEffort): string {
  return EFFORT_LABELS[effort];
}

function effortTriggerLabel(effort: ModelEffort | null | undefined, capabilities: EffortCapabilities | undefined): string {
  if (!effort) {
    if (capabilities?.support === 'supported') {
      const nativeDefault = capabilities.native_default;
      if (nativeDefault && typeof nativeDefault === 'object' && 'known' in nativeDefault) {
        return `Effort: default (${effortLabel(nativeDefault.known)})`;
      }
    }
    return 'Effort: default';
  }
  return `Effort: ${effortLabel(effort)}`;
}

function effortCompatible(capabilities: EffortCapabilities | undefined, effort: ModelEffort | null | undefined): boolean {
  if (!effort) return true;
  return capabilities?.support === 'supported' && capabilities.levels.includes(effort);
}

function StateBarPrBadge({ pr }: { pr: PrStatusResponse }) {
  if (!pr.url) return null;
  const stopPropagation = (event: ReactMouseEvent<HTMLAnchorElement>) => {
    event.stopPropagation();
  };
  return (
    <span className="pr-control">
      <a
        href={pr.url}
        target="_blank"
        rel="noreferrer"
        className={prBadgeClass(pr)}
        aria-label={`${prBadgeLabel(pr)}${pr.title ? ` · ${pr.title}` : ''}`}
        title={prTooltip(pr)}
        onClick={stopPropagation}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.stopPropagation();
          }
        }}
      >
        {prBadgeLabel(pr)}
      </a>
    </span>
  );
}

function summarizeRepo(pr: { repo_owner: string; repo_name: string }): string {
  return pr.repo_owner && pr.repo_name ? `${pr.repo_owner}/${pr.repo_name}` : 'current repo';
}

function selectorMetaLabel(pr: { head: string; base: string; repo_owner: string; repo_name: string }) {
  return `${pr.head} → ${pr.base} · ${summarizeRepo(pr)}`;
}

function autoInferenceSummary(selection: NonNullable<ConversationPrStatusHandle['activeSelection']>) {
  const observed = selection.latest_observed_branch;
  if (!observed) return 'Auto follows the latest observed branch.';
  return `Auto follows the latest observed branch: ${observed.branch_name} · ${observed.repository_identity}.`;
}

function modelLockReason(state: ConversationState): string | null {
  if (canChangeModelInState(state) || isTerminalConversationState(state)) return null;
  switch (state.type) {
    case 'awaiting_task_approval':
    case 'awaiting_commission_review_approval':
    case 'awaiting_user_response':
      return 'Model, effort, and speed are locked while this conversation awaits your response or approval.';
    default:
      return 'Model, effort, and speed are locked until the current operation settles.';
  }
}

function handleChoiceGroupKeyDown(
  event: ReactKeyboardEvent<HTMLElement>,
  selectIndex?: (index: number) => void,
) {
  if (!['ArrowDown', 'ArrowRight', 'ArrowUp', 'ArrowLeft', 'Home', 'End'].includes(event.key)) return;
  const options = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('[role="radio"]:not([disabled]), [role="option"]:not([disabled])'));
  if (options.length === 0) return;
  const currentIndex = Math.max(0, options.indexOf(document.activeElement as HTMLElement));
  let nextIndex = currentIndex;
  if (event.key === 'ArrowDown' || event.key === 'ArrowRight') nextIndex = (currentIndex + 1) % options.length;
  if (event.key === 'ArrowUp' || event.key === 'ArrowLeft') nextIndex = (currentIndex - 1 + options.length) % options.length;
  if (event.key === 'Home') nextIndex = 0;
  if (event.key === 'End') nextIndex = options.length - 1;
  event.preventDefault();
  options[nextIndex]?.focus();
  if (nextIndex !== currentIndex) selectIndex?.(nextIndex);
}

function ActivePrSelector({ handle }: { handle: ConversationPrStatusHandle }) {
  const [open, setOpen] = useState(false);
  const [focusIndex, setFocusIndex] = useState(0);
  const [pendingAction, setPendingAction] = useState<'pin' | 'resume' | null>(null);
  const [mutationError, setMutationError] = useState<string | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  const selection = handle.activeSelection;
  const activePr = handle.activePrSummary;
  const actionablePrs = selection?.associated_prs.filter((pr) => pr.display_state === 'open' || pr.display_state === 'draft') ?? [];
  const ambiguous = handle.ambiguous;
  const activeLabel = activePr ? `#${activePr.pr_number}` : 'Choose PR';
  const provenance = selection?.active_pr?.provenance;
  const pinned = provenance === 'pinned';
  const auto = provenance === 'inferred';
  const canResume = provenance === 'pinned';
  const autoSummary = selection ? autoInferenceSummary(selection) : null;
  const selectedActionableIndex = actionablePrs.findIndex((pr) =>
    activePr?.repo_owner === pr.repo_owner
    && activePr.repo_name === pr.repo_name
    && activePr.pr_number === pr.pr_number
  );

  const closeDialog = () => {
    setOpen(false);
    setMutationError(null);
    setPendingAction(null);
  };

  const openDialog = useCallback((source?: HTMLElement | null) => {
    if (source) restoreFocusRef.current = source;
    setMutationError(null);
    setOpen(true);
    setFocusIndex(selectedActionableIndex >= 0 ? selectedActionableIndex : 0);
  }, [selectedActionableIndex]);

  useEffect(() => {
    const intent: ActivePrSelectorIntent = {
      owner: Symbol('active-pr-selector-intent'),
      requestOpen: () => openDialog(document.activeElement instanceof HTMLElement ? document.activeElement : null),
    };
    return setActivePrSelectorIntent(intent);
  }, [openDialog]);

  if (actionablePrs.length === 0 && !activePr && !ambiguous) return null;

  const runPin = async (index: number) => {
    const pr = actionablePrs[index];
    if (!pr || pendingAction) return;
    setPendingAction('pin');
    setMutationError(null);
    try {
      await handle.pinActivePr?.({ repo_owner: pr.repo_owner, repo_name: pr.repo_name, pr_number: pr.pr_number });
      closeDialog();
    } catch (error) {
      setPendingAction(null);
      setMutationError(error instanceof Error ? error.message : 'Failed to set active PR');
    }
  };

  const runResume = async () => {
    if (!canResume || pendingAction) return;
    setPendingAction('resume');
    setMutationError(null);
    try {
      await handle.resumeInference?.();
      closeDialog();
    } catch (error) {
      setPendingAction(null);
      setMutationError(error instanceof Error ? error.message : 'Failed to resume automatic PR selection');
    }
  };

  return (
    <span className="active-pr-selector">
      <button
        ref={triggerRef}
        type="button"
        className={`active-pr-selector-trigger${ambiguous ? ' active-pr-selector-trigger--ambiguous' : ''}${open ? ' active-pr-selector-trigger--open' : ''}`}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={ambiguous ? 'Select active pull request' : `Active pull request ${activeLabel}`}
        data-testid="active-pr-selector-trigger"
        onClick={(event) => {
          event.stopPropagation();
          openDialog(event.currentTarget);
        }}
      >
        <span className="active-pr-selector-label">{ambiguous ? 'PR ?' : activeLabel}</span>
        {pinned && <span className="active-pr-selector-badge" data-testid="active-pr-pinned-indicator">Pinned</span>}
        {auto && activePr && <span className="active-pr-selector-badge">Auto</span>}
      </button>
      {open && (
        <SelectionDialog
          title="Choose active pull request"
          description={ambiguous ? 'Multiple actionable PRs are associated with this work. Choose the one Phoenix should target.' : 'Choose which pull request Phoenix should target for this work.'}
          onClose={closeDialog}
          dismissible={pendingAction === null}
          restoreFocusRef={restoreFocusRef}
          ariaBusy={pendingAction !== null}
          className="active-pr-dialog"
        >
          {autoSummary && auto && (
            <div className="active-pr-selector-auto-summary" data-testid="active-pr-auto-summary">{autoSummary}</div>
          )}
          <div className="active-pr-selector-options" role="listbox" aria-label="Active pull request choices" onKeyDown={(event) => handleChoiceGroupKeyDown(event, setFocusIndex)}>
            {actionablePrs.map((pr, index) => {
              const isActive = activePr?.repo_owner === pr.repo_owner
                && activePr.repo_name === pr.repo_name
                && activePr.pr_number === pr.pr_number;
              return (
                <button
                  key={`${pr.repo_owner}/${pr.repo_name}#${pr.pr_number}`}
                  type="button"
                  role="option"
                  aria-selected={isActive}
                  className={`active-pr-selector-item${isActive ? ' active-pr-selector-item--active' : ''}`}
                  data-testid={`active-pr-choice-${pr.pr_number}`}
                  data-selection-dialog-autofocus={index === focusIndex ? '' : undefined}
                  tabIndex={index === focusIndex ? 0 : -1}
                  disabled={pendingAction !== null}
                  onClick={() => void runPin(index)}
                >
                  <span className="active-pr-selector-item-main">
                    <span className="active-pr-selector-item-line">
                      <span className="active-pr-selector-item-label">#{pr.pr_number}</span>
                      <span className="active-pr-selector-item-title">{pr.title}</span>
                      <span className="active-pr-selector-item-state">{pr.display_state === 'draft' ? 'Draft' : 'Open'}</span>
                      {isActive && <span className="active-pr-selector-item-state">Active</span>}
                    </span>
                    <span className="active-pr-selector-item-meta">{selectorMetaLabel(pr)}</span>
                  </span>
                </button>
              );
            })}
          </div>
          {canResume && (
            <div className="active-pr-selector-auto-action">
              <button
                type="button"
                className="active-pr-selector-item active-pr-selector-item--resume"
                data-testid="active-pr-resume-inference"
                disabled={pendingAction !== null}
                onClick={() => void runResume()}
              >
                <span className="active-pr-selector-item-main">
                  <span className="active-pr-selector-item-line active-pr-selector-item-line--resume">
                    <span className="active-pr-selector-item-title">{pendingAction === 'resume' ? 'Resuming automatic selection…' : 'Resume automatic selection'}</span>
                  </span>
                  {selection && <span className="active-pr-selector-item-meta">{autoInferenceSummary(selection)}</span>}
                </span>
              </button>
            </div>
          )}
          {pendingAction && <div className="active-pr-selector-status" role="status">Saving active PR…</div>}
          {mutationError && <div className="active-pr-selector-error" role="alert">{mutationError}</div>}
        </SelectionDialog>
      )}
    </span>
  );
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
  terminalLauncher,
  prStatusHandle,
  workActionsAvailable = true,
}: StateBarProps) {
  // `toolExecutingStartedAt` is kept on the prop type for the
  // tool-widget header (which still reads it from the atom). The
  // StateBar's own elapsed counter switched to `phaseStateUpdatedAt`
  // in Stage A — the destructured value is intentionally unused here.
  void _deprecatedToolStartedAt;
  const [pickerOpen, setPickerOpen] = useState(false);
  const [pickerConversationId, setPickerConversationId] = useState<string>();
  const [pickerShowAll, setPickerShowAll] = useState(false);
  const [stagedModel, setStagedModel] = useState('');
  const [stagedEffort, setStagedEffort] = useState<ModelEffort | null>(null);
  const [stagedServiceTier, setStagedServiceTier] = useState<ServiceTier>('standard');
  const [modelMutationPending, setModelMutationPending] = useState(false);
  const [modelMutationError, setModelMutationError] = useState<string | null>(null);
  const usesCompactLayout = useIsCompactLayout();
  const [mobileExpanded, setMobileExpanded] = useState(false);
  // Collapse the mobile-expanded section when the viewport widens past
  // mobile — otherwise a user who expanded on phone, rotated to landscape,
  // would see a desktop bar with a stale "expanded" affordance.
  useEffect(() => {
    if (!usesCompactLayout) setMobileExpanded(false);
  }, [usesCompactLayout]);
  const pickerTriggerRef = useRef<HTMLButtonElement>(null);
  const conversationIdentityRef = useRef(conversation?.id);
  useEffect(() => {
    conversationIdentityRef.current = conversation?.id;
    setPickerOpen(false);
    setPickerShowAll(false);
    setPickerConversationId(undefined);
    setStagedModel('');
    setStagedEffort(null);
    setStagedServiceTier('standard');
    setModelMutationPending(false);
    setModelMutationError(null);
  }, [conversation?.id]);

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
    setPhaseElapsedSeconds(
      Math.max(0, Math.floor((Date.now() - phaseStateUpdatedAt) / 1000)),
    );
    const interval = window.setInterval(() => {
      setPhaseElapsedSeconds(
        Math.max(0, Math.floor((Date.now() - phaseStateUpdatedAt) / 1000)),
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
    (connectionState === "connected" || connectionState === "reconnected") &&
    phaseIsWorking &&
    typeof lastSseEventAtRef?.current === "number";
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
      const elapsed = Math.max(
        0,
        Math.floor((Date.now() - lastSseEventAtRef!.current) / 1000),
      );
      setWatchdogSeconds(elapsed);
    };
    compute();
    const interval = window.setInterval(compute, 1000);
    return () => window.clearInterval(interval);
  }, [watchdogArmed, lastSseEventAtRef]);
  const watchdogStale =
    watchdogArmed && watchdogSeconds >= HEARTBEAT_WATCHDOG_SECONDS;

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
    connectionState === "connected" || connectionState === "reconnected";
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
        (convState.type === "llm_requesting" ||
          convState.type === "seeded_llm_requesting" ||
          convState.type === "awaiting_llm") &&
        firstByteRequestId != null;
      lastKnownActivityRef.current = {
        phase: convState,
        elapsedSecondsAtDisconnect: Math.max(
          0,
          Math.floor((Date.now() - phaseStateUpdatedAt) / 1000),
        ),
        wasStreaming,
      };
    }
    // No-op on subsequent renders during the same degraded window —
    // the snapshot is by-construction frozen.
  }, [
    connectionHealthy,
    phaseIsWorking,
    phaseStateUpdatedAt,
    convState,
    firstByteRequestId,
  ]);

  let dotClass = "dot";
  let stateText = "";

  // REQ-WPV-003 / REQ-LRV-001: when a retry has fired this turn, append
  // "(retry K/N after <reason>)" to the base reason. Returns "" when no
  // retry context exists. Leading space so the suffix concatenates
  // cleanly onto either the live or the frozen-last-known reason.
  const retrySuffix =
    turnRetryContext != null
      ? ` (retry ${turnRetryContext.attempt}/${turnRetryContext.maxAttempts} after ${turnRetryContext.reasonText})`
      : "";

  // Format the working-phase reason as "<base> Ns <retry?>" (e.g.
  // "awaiting LLM response 4s (retry 2/3 after rate limit)",
  // "running bash 12s") for use in both the live and the frozen-last-
  // known-activity paths below.
  const formatWorkingReason = (
    phase: ConversationState,
    elapsedSeconds: number,
    streaming = false,
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
    const base = getStateDescription(phase).replace(/\.{3}$/, "");
    const withElapsed =
      elapsedSeconds > 0
        ? `${base} ... ${formatElapsed(elapsedSeconds)}`
        : base;
    return `${withElapsed}${retrySuffix}`;
  };

  if (!conversation) {
    dotClass += " hidden";
    stateText = "";
  } else if (watchdogStale) {
    // REQ-WPV-004: degraded-signal indicator overrides every working-
    // state message. The user needs to know the channel is suspect
    // before they trust further detail.
    dotClass += " degraded";
    stateText = `no signal from server for ${formatElapsed(watchdogSeconds)}`;
  } else {
    // Determine dot and text based on connection state first.
    switch (connectionState) {
      case "disconnected":
        dotClass += " connecting";
        stateText = "connecting...";
        break;

      case "connecting":
        dotClass += " connecting";
        stateText = "connecting...";
        break;

      case "reconnecting": {
        // REQ-WPV-005: don't mask agent activity. If we were working
        // when we degraded, show both the connection chip AND the
        // last-known agent activity with its elapsed FROZEN at
        // disconnect (honest about what we don't know — the agent
        // may or may not still be doing the thing).
        dotClass += " reconnecting";
        const snap = lastKnownActivityRef.current;
        if (snap) {
          stateText = `reconnecting (${connectionAttempt}) — last: ${formatWorkingReason(
            snap.phase,
            snap.elapsedSecondsAtDisconnect,
            snap.wasStreaming,
          )}`;
        } else {
          stateText = `reconnecting (${connectionAttempt})...`;
        }
        break;
      }

      case "offline": {
        // Same composition as reconnecting; the connection-chip text
        // changes from "reconnecting (N)" to "offline" but the
        // last-known activity is still surfaced when we have it.
        dotClass += " offline";
        const snap = lastKnownActivityRef.current;
        if (snap) {
          stateText = `offline — last: ${formatWorkingReason(
            snap.phase,
            snap.elapsedSecondsAtDisconnect,
            snap.wasStreaming,
          )}`;
        } else {
          stateText = "offline";
        }
        break;
      }

      case "reconnected":
        dotClass += " reconnected";
        stateText = "reconnected";
        break;

      case "connected": {
        // When connected, show agent state
        switch (convState.type) {
          case "idle":
            dotClass += " idle";
            stateText = "ready";
            break;
          case "terminal":
          case "handed_off":
          case "creation_cancelled":
            dotClass += " terminal";
            stateText = convState.type === "handed_off"
              ? "handed off"
              : convState.type === "creation_cancelled"
                ? "creation cancelled"
                : "completed";
            break;
          case 'awaiting_task_approval':
          case 'awaiting_commission_review_approval':
            dotClass += ' approval';
            stateText = 'awaiting approval';
            break;
          case "awaiting_user_response":
            dotClass += " approval";
            // "your reply" disambiguates from `awaiting LLM response`
            // (the llm_requesting label). Both surface in the same
            // StateBar slot; the prose has to make clear who the user
            // is waiting on.
            stateText = "awaiting your reply";
            break;
          case "error":
            dotClass += " error";
            stateText = "error";
            break;
          case "recoverable_continuation_failure":
            dotClass += " error";
            stateText = "continuation failed";
            break;
          case "creation_failed":
            dotClass += " error";
            stateText = "creation failed";
            break;
          case "context_exhausted":
            dotClass += " error";
            stateText = "context full";
            break;
          case "awaiting_llm":
          case "llm_requesting":
          case "seeded_llm_requesting":
          case "tool_executing":
          case "awaiting_sub_agents":
          case "awaiting_continuation":
          case "cancelling":
          case "cancelling_tool":
          case "cancelling_sub_agents":
          case "awaiting_recovery":
          case "provisioning":
            // REQ-WPV-001/003: elapsed counter is keyed off the
            // server-authoritative `phaseStateUpdatedAt`, so every
            // working phase gets a live "<reason> Ns" display (not
            // just tool_executing). The previous tool-only counter
            // is retained on the tool widget header itself via
            // `toolExecutingStartedAt`.
            dotClass += " working";
            // REQ-WPV-007: once the first byte for the current LLM
            // request lands, switch the base reason from `awaiting LLM response Ns`
            // to `streaming` (no counter — the stream itself is the
            // progress signal). The transition applies only while
            // the phase is one of the llm_requesting family; tool/
            // sub-agent phases retain their elapsed counter.
            if (
              (convState.type === "llm_requesting" ||
                convState.type === "seeded_llm_requesting" ||
                convState.type === "awaiting_llm") &&
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
          default:
            convState satisfies never;
        }
        break;
      }

      default:
        dotClass += " connecting";
        stateText = "connecting...";
    }
  }

  const showOfflineBanner =
    connectionState === "offline" && nextRetryIn !== null;

  // Context window indicator -- use model-specific limit, fallback for legacy
  const maxTokens = modelContextWindow || 200_000;
  // Trigger menu only available in idle phase (the indicator gates on the
  // context threshold itself). The capability is structurally inseparable
  // from the idle discriminant, so no `convState.type` re-check is needed
  // here — narrowing the discriminated union is the guard (task 04001).
  const indicatorTrigger =
    continuation?.phase === "idle" ? continuation.onTrigger : undefined;

  // Derived display values
  const identity = conversation ? getConversationIdentity(conversation) : null;
  const mode = identity?.mode.key ?? conversation?.conv_mode_label?.toLowerCase() ?? 'unknown';
  const isWork = mode === "work";
  const isExplore = mode === "explore";
  const isBranchMode = mode === "branch";
  const modeLabel = identity?.mode.label ?? null;
  const modeSuffix = isExplore ? " (read-only)" : "";
  const modeClass = `statebar-mode statebar-mode--${mode}`;
  const modelAbbrev = identity
    ? abbreviateModel(identity.modelLabel ?? "")
    : "";

  // Model picker: available when no operation is in flight (idle or error)
  // and we have models and a callback. Error-state switch lets the user
  // recover from overload/quota by picking another model, then retrying.
  const currentModel = conversation?.model ?? "";
  const currentModelInfo = availableModels?.find((model) => model.id === currentModel);
  const currentEffortCapabilities = currentModelInfo?.effort_capabilities;
  const persistedEffort = conversation?.effort ?? null;
  const effortIsStale = persistedEffort !== null
    && !effortCompatible(currentEffortCapabilities, persistedEffort);
  const currentEffort = persistedEffort;
  const currentServiceTier = conversation?.service_tier ?? 'standard';
  const supportsFastMode = currentModelInfo?.service_tier_capabilities === 'supported';
  const canPickModel = !!(
    onUpgradeModel &&
    availableModels &&
    availableModels.length > 0 &&
    canChangeModelInState(convState)
  );

  useEffect(() => {
    if (canPickModel || !pickerOpen) return;
    setPickerOpen(false);
    setStagedModel('');
    setStagedEffort(null);
    setStagedServiceTier('standard');
    setModelMutationError(null);
  }, [canPickModel, pickerOpen]);

  const pickerModels: ModelInfo[] = (() => {
    if (!availableModels) return [];
    if (pickerShowAll) return availableModels;
    const recommended = availableModels.filter((model) => model.recommended);
    const selectedId = stagedModel || currentModel;
    if (selectedId && !recommended.some((model) => model.id === selectedId)) {
      const selected = availableModels.find((model) => model.id === selectedId);
      if (selected) return [selected, ...recommended];
    }
    return recommended;
  })();
  const stagedModelInfo = availableModels?.find((model) => model.id === stagedModel);
  const stagedEffortCapabilities = stagedModelInfo?.effort_capabilities;
  const stagedSupportsFastMode = stagedModelInfo?.service_tier_capabilities === 'supported';
  const modelSelectionChanged = stagedModel !== currentModel
    || stagedEffort !== persistedEffort
    || stagedServiceTier !== currentServiceTier;
  const currentModelLockReason = onUpgradeModel && availableModels && availableModels.length > 0
    ? modelLockReason(convState)
    : null;

  const openModelDialog = () => {
    if (!canPickModel) return;
    setStagedModel(currentModel);
    setStagedEffort(effortCompatible(currentEffortCapabilities, persistedEffort) ? persistedEffort : null);
    setStagedServiceTier(supportsFastMode ? currentServiceTier : 'standard');
    setModelMutationError(null);
    setPickerOpen(true);
    setPickerConversationId(conversation?.id);
  };

  const closeModelDialog = () => {
    if (modelMutationPending) return;
    setPickerOpen(false);
    setModelMutationError(null);
    setPickerConversationId(undefined);
  };

  const stageModel = (modelId: string) => {
    const targetModel = availableModels?.find((model) => model.id === modelId);
    setStagedModel(modelId);
    setStagedEffort((effort) => effortCompatible(targetModel?.effort_capabilities, effort) ? effort : null);
    setStagedServiceTier((tier) => targetModel?.service_tier_capabilities === 'supported' ? tier : 'standard');
    setModelMutationError(null);
  };

  const applyModelSelection = async () => {
    if (!onUpgradeModel || !stagedModel || !modelSelectionChanged || modelMutationPending) return;
    const submittedConversationId = conversation?.id;
    setModelMutationPending(true);
    setModelMutationError(null);
    try {
      await onUpgradeModel(stagedModel, stagedEffort, stagedServiceTier);
      if (conversationIdentityRef.current === submittedConversationId) {
        setPickerOpen(false);
        setPickerConversationId(undefined);
      }
    } catch (error) {
      if (conversationIdentityRef.current === submittedConversationId) {
        setModelMutationError(error instanceof Error ? error.message : 'Failed to update model, effort, and speed');
      }
    } finally {
      if (conversationIdentityRef.current === submittedConversationId) setModelMutationPending(false);
    }
  };

  const baseBranch = identity?.branch.base ?? null;
  const branchName = identity?.branch.active ?? null;
  const taskTitle = identity?.taskTitle ?? null;
  const projectName = identity?.projectLabel ?? null;
  const prStatus =
    prStatusHandle?.state.status === "ready" ? prStatusHandle.state.prStatus : null;
  const prLoading = prStatusHandle?.state.status === "loading";
  const prHint =
    prStatus && !prStatus.found
      ? unavailablePrHint(prStatus.unavailable_reason)
      : null;
  const prRailAvailability = prStatusHandle
    ? derivePrRailAvailability(prStatusHandle, usesCompactLayout)
    : null;
  const workActionsPrRailOwnsSelection = Boolean(
    workActionsAvailable
    && (isWork || isBranchMode)
    && ['idle', 'error', 'recoverable_continuation_failure', 'context_exhausted'].includes(convState.type)
    && prRailAvailability?.shouldRender
  );

  const cwdSummary = identity?.path.summary ?? '—';
  const displayedPath = identity?.path.full ?? null;
  const modeHelp = identity?.mode.title ?? 'Full access';
  const modeDetail = identity?.mode.detail ?? 'Full access';

  const selectorHasContent = Boolean(
    prStatusHandle
    && (
      prStatusHandle.activePrSummary
      || prStatusHandle.ambiguous
      || prStatusHandle.activeSelection?.associated_prs.some(
        (pr) => pr.display_state === 'open' || pr.display_state === 'draft',
      )
    ),
  );
  const prStatusContent = (
    <>
      {prStatus && prStatus.found && prStatus.url && <StateBarPrBadge pr={prStatus} />}
      {!workActionsPrRailOwnsSelection && prStatusHandle && <ActivePrSelector handle={prStatusHandle} />}
      {prHint && !prLoading && (
        <span
          className="pr-hint"
          title="Install and authenticate GitHub CLI to enable PR tracking"
        >
          {prHint}
        </span>
      )}
    </>
  );
  const hasPrContent = Boolean(
    (prStatus?.found && prStatus.url)
    || (!workActionsPrRailOwnsSelection && selectorHasContent)
    || (prHint && !prLoading),
  );

  const renderModelControl = (variant: "desktop" | "mobile" = "desktop") => (
    <span className={`conv-model-wrapper${variant === "mobile" ? " conv-model-wrapper--mobile" : ""}`}>
      {canPickModel ? (
        <button
          ref={pickerTriggerRef}
          type="button"
          className="conv-model conv-model--button"
          title={`Model: ${conversation?.model ?? "default"} (click to change)`}
          onClick={(event) => {
            event.stopPropagation();
            openModelDialog();
          }}
          aria-haspopup="dialog"
          aria-expanded={pickerOpen}
        >
          <span className="conv-model-value">
            {variant === "mobile" ? (conversation?.model ?? "default") : modelAbbrev}
            {(currentEffortCapabilities?.support !== 'unsupported' || effortIsStale) && (
              <span className="conv-model-effort"> · {currentEffort ? `${effortLabel(currentEffort)}${effortIsStale ? ' (unsupported)' : ''}` : effortTriggerLabel(null, currentEffortCapabilities).replace('Effort: ', '')}</span>
            )}
            {currentServiceTier === 'fast' && <span className="conv-model-effort"> · Fast</span>}
          </span>
          <span className="conv-model-caret" aria-hidden="true">&#9662;</span>
        </button>
      ) : (
        <span className="conv-model-readonly">
          <span className="conv-model" title={`Model: ${conversation?.model ?? "default"}`}>
            {variant === "mobile" ? (conversation?.model ?? "default") : modelAbbrev}
            {(currentEffortCapabilities?.support !== 'unsupported' || effortIsStale) && (
              <span className="conv-model-effort"> · {currentEffort ? `${effortLabel(currentEffort)}${effortIsStale ? ' (unsupported)' : ''}` : effortTriggerLabel(null, currentEffortCapabilities).replace('Effort: ', '')}</span>
            )}
            {currentServiceTier === 'fast' && <span className="conv-model-effort"> · Fast</span>}
          </span>
          {variant === "mobile" && onUpgradeModel && availableModels && availableModels.length > 0 && currentModelLockReason && (
            <span className="conv-model-lock-reason">{currentModelLockReason}</span>
          )}
        </span>
      )}
      {pickerOpen && pickerConversationId === conversation?.id && canPickModel && (
        <SelectionDialog
          title="Model, effort, and speed"
          description="Choose the model, reasoning effort, and request speed for the next turn. Changes apply together."
          onClose={closeModelDialog}
          dismissible={!modelMutationPending}
          restoreFocusRef={pickerTriggerRef}
          ariaBusy={modelMutationPending}
          className="model-selection-dialog"
          footer={
            <>
              <button type="button" className="selection-dialog__cancel" onClick={closeModelDialog} disabled={modelMutationPending}>Cancel</button>
              <button type="button" className="selection-dialog__apply" onClick={() => void applyModelSelection()} disabled={!modelSelectionChanged || modelMutationPending}>
                {modelMutationPending ? 'Applying…' : 'Apply'}
              </button>
            </>
          }
        >
          <fieldset className="model-picker-section" disabled={modelMutationPending}>
            <legend>Model</legend>
            <div className="model-picker-list" role="radiogroup" aria-label="Select model" onKeyDown={(event) => handleChoiceGroupKeyDown(event, (index) => { const model = pickerModels[index]; if (model) stageModel(model.id); })}>
              {pickerModels.map((model, index) => {
                const selected = model.id === stagedModel;
                return (
                  <button
                    key={model.id}
                    type="button"
                    role="radio"
                    aria-checked={selected}
                    className={`model-picker-item${selected ? " model-picker-item--selected" : ""}`}
                    onClick={() => stageModel(model.id)}
                    title={model.description || model.id}
                    data-selection-dialog-autofocus={selected || (!stagedModel && index === 0) ? '' : undefined}
                    tabIndex={selected || (!stagedModel && index === 0) ? 0 : -1}
                  >
                    <span className="model-picker-item-check" aria-hidden="true">{selected ? <CheckIcon /> : null}</span>
                    <span className="model-picker-item-main">
                      <span className="model-picker-item-id">{model.id}</span>
                      {model.description && <span className="model-picker-item-description">{model.description}</span>}
                    </span>
                    <span className="model-picker-item-ctx">{formatContextWindow(model.context_window)}</span>
                  </button>
                );
              })}
            </div>
            <label className="model-picker-show-all-toggle">
              <input type="checkbox" checked={pickerShowAll} onChange={(event) => setPickerShowAll(event.target.checked)} />
              <span>Show all models</span>
            </label>
          </fieldset>
          {stagedEffortCapabilities?.support === 'supported' ? (
            <fieldset className="model-picker-section" disabled={modelMutationPending}>
              <legend>Effort</legend>
              <div className="model-picker-list model-picker-effort-list" role="radiogroup" aria-label="Select effort" onKeyDown={(event) => handleChoiceGroupKeyDown(event, (index) => setStagedEffort(index === 0 ? null : stagedEffortCapabilities.levels[index - 1] ?? null))}>
                <button
                  type="button"
                  role="radio"
                  aria-checked={stagedEffort === null}
                  tabIndex={stagedEffort === null ? 0 : -1}
                  className={`model-picker-item${stagedEffort === null ? ' model-picker-item--selected' : ''}`}
                  onClick={() => setStagedEffort(null)}
                >
                  <span className="model-picker-item-check" aria-hidden="true">{stagedEffort === null ? <CheckIcon /> : null}</span>
                  <span className="model-picker-item-id">{effortTriggerLabel(null, stagedEffortCapabilities)}</span>
                </button>
                {stagedEffortCapabilities.levels.map((level) => {
                  const selected = stagedEffort === level;
                  return (
                    <button
                      key={level}
                      type="button"
                      role="radio"
                      aria-checked={selected}
                      tabIndex={selected ? 0 : -1}
                      className={`model-picker-item${selected ? ' model-picker-item--selected' : ''}`}
                      onClick={() => setStagedEffort(level)}
                    >
                      <span className="model-picker-item-check" aria-hidden="true">{selected ? <CheckIcon /> : null}</span>
                      <span className="model-picker-item-id">{effortLabel(level)}</span>
                    </button>
                  );
                })}
              </div>
            </fieldset>
          ) : stagedEffortCapabilities?.support === 'unknown' ? (
            <div className="model-picker-capability-note">Effort controls are unavailable because this model's effort capabilities are unknown.</div>
          ) : null}
          {stagedSupportsFastMode && (
            <fieldset className="model-picker-section" disabled={modelMutationPending}>
              <legend>Speed</legend>
              <div className="model-picker-list model-picker-speed-list" role="radiogroup" aria-label="Select speed" onKeyDown={(event) => handleChoiceGroupKeyDown(event, (index) => setStagedServiceTier(index === 1 ? 'fast' : 'standard'))}>
                <button
                  type="button"
                  role="radio"
                  aria-checked={stagedServiceTier === 'standard'}
                  tabIndex={stagedServiceTier === 'standard' ? 0 : -1}
                  className={`model-picker-item${stagedServiceTier === 'standard' ? ' model-picker-item--selected' : ''}`}
                  onClick={() => setStagedServiceTier('standard')}
                >
                  <span className="model-picker-item-check" aria-hidden="true">{stagedServiceTier === 'standard' ? <CheckIcon /> : null}</span>
                  <span className="model-picker-item-main">
                    <span className="model-picker-item-id">Standard</span>
                    <span className="model-picker-item-description">Standard speed and usage</span>
                  </span>
                </button>
                <button
                  type="button"
                  role="radio"
                  aria-checked={stagedServiceTier === 'fast'}
                  tabIndex={stagedServiceTier === 'fast' ? 0 : -1}
                  className={`model-picker-item${stagedServiceTier === 'fast' ? ' model-picker-item--selected' : ''}`}
                  onClick={() => setStagedServiceTier('fast')}
                >
                  <span className="model-picker-item-check" aria-hidden="true">{stagedServiceTier === 'fast' ? <CheckIcon /> : null}</span>
                  <span className="model-picker-item-main">
                    <span className="model-picker-item-id">Fast</span>
                    <span className="model-picker-item-description">Approximately 1.5x speed, increased usage</span>
                  </span>
                </button>
              </div>
            </fieldset>
          )}
          {modelMutationError && <div className="model-picker-error" role="alert">{modelMutationError}</div>}
        </SelectionDialog>
      )}
    </span>
  );

  const renderFilesButton = () =>
    onOpenFiles ? (
      <button
        type="button"
        className="statebar-files-btn"
        onClick={(e) => {
          e.stopPropagation();
          onOpenFiles();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.stopPropagation();
          }
        }}
        aria-label="Browse project files"
        title="Browse project files"
      >
        <FolderTree size={18} aria-hidden="true" />
      </button>
    ) : null;

  const terminalLauncherAccessibleName = terminalLauncher
    ? `Open terminal, ${terminalLauncher.status.activity}, ${terminalLauncher.status.cwd || 'Shell'}${
      terminalLauncher.status.unreadLines > 0
        ? `, ${terminalLauncher.status.unreadLines} unread lines`
        : ''
    }`
    : undefined;

  if (usesCompactLayout) {
    const toggleMobileExpanded = () => setMobileExpanded((v) => !v);
    const handleCollapsedKey = (e: ReactKeyboardEvent) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        setMobileExpanded(true);
      }
    };

    return (
      <>
        <header
          id="state-bar"
          className={`statebar-mobile ${mobileExpanded ? "statebar-mobile-expanded" : "statebar-mobile-collapsed"}`}
          role={!mobileExpanded ? "button" : undefined}
          tabIndex={!mobileExpanded ? 0 : undefined}
          aria-expanded={mobileExpanded}
          aria-label={!mobileExpanded ? "Expand status bar" : undefined}
          onClick={!mobileExpanded ? () => setMobileExpanded(true) : undefined}
          onKeyDown={!mobileExpanded ? handleCollapsedKey : undefined}
        >
          <div className="statebar-mobile-primary">
            {conversation ? (
              <Link
                to="/"
                className="statebar-slug statebar-mobile-slug"
                title="Back to conversations"
                onClick={(e) => e.stopPropagation()}
              >
                <span className="back-arrow">&larr;</span>
                <span className="slug-text">{conversation.slug}</span>
              </Link>
            ) : (
              <span className="statebar-slug">&mdash;</span>
            )}
            <div className="statebar-mobile-status" title={stateText}>
              <span className={dotClass}></span>
              {!mobileExpanded && (
                <span className="state-text">{stateText}</span>
              )}
            </div>
            <div className="statebar-mobile-actions">
              {renderFilesButton()}
              <button
                type="button"
                className="statebar-chevron"
                onClick={(e) => {
                  e.stopPropagation();
                  toggleMobileExpanded();
                }}
                aria-label={
                  mobileExpanded ? "Collapse status bar" : "Expand status bar"
                }
                aria-expanded={mobileExpanded}
              >
                {mobileExpanded ? "▾" : "▴"}
              </button>
            </div>
          </div>

          {mobileExpanded && conversation && (
            <div className="statebar-mobile-details">
              <section
                className="statebar-mobile-section"
                aria-label="Working directory"
              >
                <span className="statebar-mobile-label">Path</span>
                <code
                  className="statebar-mobile-value statebar-mobile-path"
                  title={displayedPath ?? undefined}
                >
                  {cwdSummary}
                </code>
                {displayedPath && (
                  <button
                    type="button"
                    className="statebar-mobile-copy"
                    onClick={() =>
                      void navigator.clipboard?.writeText(displayedPath)
                    }
                    aria-label={`Copy full working directory ${displayedPath}`}
                    title={displayedPath}
                  >
                    Copy cwd
                  </button>
                )}
              </section>

              <section
                className="statebar-mobile-section"
                aria-label="Conversation identity"
              >
                {modeLabel && (
                  <div className="statebar-mobile-row">
                    <span className="statebar-mobile-label">Mode</span>
                    <span className={modeClass} title={modeHelp}>
                      {modeLabel}
                      {modeSuffix}
                    </span>
                    <span className="statebar-mobile-help">{modeDetail}</span>
                  </div>
                )}
                <div className="statebar-mobile-row">
                  <span className="statebar-mobile-label">Model</span>
                  {renderModelControl("mobile")}
                </div>
                {taskTitle && (
                  <div className="statebar-mobile-row">
                    <span className="statebar-mobile-label">Task</span>
                    <span className="statebar-mobile-value" title={taskTitle}>
                      {taskTitle}
                    </span>
                  </div>
                )}
              </section>

              {branchName && (
                <section
                  className="statebar-mobile-section"
                  aria-label="Branch and pull request"
                >
                  <span className="statebar-mobile-label">Branch</span>
                  <code
                    className="statebar-mobile-value statebar-mobile-branch"
                    title={branchName}
                  >
                    {branchName}
                  </code>
                  <span className="statebar-mobile-pr">{prStatusContent}</span>
                </section>
              )}

              {terminalLauncher && (
                <section
                  className="statebar-mobile-section statebar-mobile-section--terminal"
                  aria-label="Terminal"
                >
                  <button
                    ref={terminalLauncher.buttonRef}
                    type="button"
                    className="statebar-terminal-launcher"
                    onClick={(event) => {
                      event.stopPropagation();
                      terminalLauncher.onOpen();
                    }}
                    aria-label={terminalLauncherAccessibleName}
                  >
                    <TerminalSquare size={20} aria-hidden="true" />
                    <span className="statebar-terminal-launcher-copy">
                      <span className="statebar-terminal-launcher-title">
                        Terminal
                        <span
                          className={`statebar-terminal-dot statebar-terminal-dot--${terminalLauncher.status.activity}`}
                          aria-hidden="true"
                        />
                      </span>
                      <code className="statebar-terminal-launcher-path" title={terminalLauncher.status.cwd}>
                        {terminalLauncher.status.cwd || 'Shell'}
                      </code>
                    </span>
                    {terminalLauncher.status.unreadLines > 0 && (
                      <span className="statebar-terminal-unread">
                        +{terminalLauncher.status.unreadLines}
                      </span>
                    )}
                    <span className="statebar-terminal-arrow" aria-hidden="true">›</span>
                  </button>
                </section>
              )}

              <section
                className="statebar-mobile-section statebar-mobile-section--runtime"
                aria-label="Runtime and context"
              >
                <div className="statebar-mobile-runtime">
                  <span className={dotClass}></span>
                  <span className="state-text">{stateText}</span>
                </div>
                {contextWindowUsed > 0 && (
                  <ContextIndicator
                    used={contextWindowUsed}
                    max={maxTokens}
                    conversationId={conversation.id}
                    onTriggerContinuation={indicatorTrigger}
                  />
                )}
              </section>
            </div>
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

  return (
    <>
      <header id="state-bar">
        <div id="state-bar-left">
          {conversation ? (
            <>
              {/* Line 1: nav slug + mode + model */}
              <div className="statebar-line1">
                <Link
                  to="/"
                  className="statebar-slug"
                  title="Back to conversations"
                >
                  <span className="back-arrow">&larr;</span>
                  <span className="slug-text">{conversation.slug}</span>
                </Link>
                {modeLabel && (
                  <span
                    className={modeClass}
                    title={modeHelp}
                  >
                    {modeLabel}
                    {modeSuffix}
                  </span>
                )}
                <span className="statebar-desktop-heading" title={identity?.title ?? conversation.slug}>
                  {identity?.title ?? conversation.slug}
                </span>
                {projectName && (
                  <span className="statebar-project" title={identity?.path.full ?? conversation.cwd}>
                    {projectName}
                  </span>
                )}
                <span className="statebar-desktop-mode-detail" title={modeHelp}>{modeDetail}</span>
                {renderModelControl("desktop")}
              </div>

              {(taskTitle || branchName || hasPrContent) && (
                <div className="statebar-line2">
                  {taskTitle && taskTitle !== identity?.title && (
                    <span
                      className="statebar-task-title"
                      title={taskTitle}
                    >
                      {taskTitle}
                    </span>
                  )}
                  {branchName && (
                    <span className="git-branch-block" title={branchName}>
                      <span className="git-label">Branch</span>
                      <span className="git-branch">{branchName}</span>
                    </span>
                  )}
                  {branchName && baseBranch && (
                    <span className="git-base-block" title={baseBranch}>
                      <span className="git-label">from</span>
                      <span className="git-base">{baseBranch}</span>
                    </span>
                  )}
                  {hasPrContent && <span className="statebar-pr-slot">{prStatusContent}</span>}
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
                if (e.key === "Enter" || e.key === " ") {
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
        {usesCompactLayout && (
          <button
            type="button"
            className="statebar-chevron"
            onClick={(e) => {
              e.stopPropagation();
              setMobileExpanded((v) => !v);
            }}
            aria-label={
              mobileExpanded ? "Collapse status bar" : "Expand status bar"
            }
            aria-expanded={mobileExpanded}
          >
            {mobileExpanded ? "▾" : "▴"}
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
}: Omit<StateBarProps, "lastSseEventAtRef"> & { slug: string }) {
  const lastSseEventAtRef = useLastSseEventAtRef(slug);
  return <StateBar {...rest} lastSseEventAtRef={lastSseEventAtRef} />;
}
