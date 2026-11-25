import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { Link } from "react-router-dom";
import type { PrStatusResponse } from "../api";
import { useProductConversationList } from "../conversation/productConversationList";
import type { ConversationPrStatusHandle } from "../hooks/useConversationPrStatus";
import { SelectionDialog } from "./SelectionDialog";
import { setActivePrSelectorIntent, type ActivePrSelectorIntent } from "./activePrSelectorIntent";
import { prBadgeClass, prBadgeLabel, prTooltip } from "./prBadge";

// eslint-disable-next-line react-refresh/only-export-components
export function handleChoiceGroupKeyDown(
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

export function StateBarMonitoredBadge({ productConversationId }: { productConversationId: string | undefined }) {
  const { rows } = useProductConversationList();
  const monitored = productConversationId !== undefined
    && rows.some((row) => row.product_conversation_id === productConversationId && row.monitored);
  if (!monitored) return null;
  return (
    <Link
      to="/global"
      className="statebar-monitored"
      title="Ember is monitoring this conversation and directs its work. Open Ember."
    >
      Ember is handling this
    </Link>
  );
}

export function StateBarPrBadge({ pr }: { pr: PrStatusResponse }) {
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

export function ActivePrSelector({ handle }: { handle: ConversationPrStatusHandle }) {
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
    if (actionablePrs.length === 0) {
      setFocusIndex(0);
      return;
    }
    setFocusIndex((current) => Math.min(current, actionablePrs.length - 1));
  }, [actionablePrs.length]);

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
