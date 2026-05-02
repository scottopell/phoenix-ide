import { useState, useRef, useEffect, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { Link } from 'react-router-dom';
import type { Conversation, ConversationState, ModelInfo } from '../api';
import type { ConnectionState } from '../hooks';
import { getStateDescription } from '../utils';
import { ContextIndicator } from './ContextIndicator';

const CheckIcon = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <polyline points="20 6 9 17 4 12" />
  </svg>
);

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
  /** Callback to manually trigger continuation */
  onTriggerContinuation?: () => void;
  /** Callback invoked when the user selects a different model for this conversation */
  onUpgradeModel?: (newModelId: string) => void;
  /** `Date.now()` timestamp when the current tool_executing phase began.
   *  Used to render a live elapsed-time counter ("running bash ... 4s").
   *  `null` or `undefined` when not in tool_executing. */
  toolExecutingStartedAt?: number | null;
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

/** Abbreviate model ID: "claude-sonnet-4-6" -> "sonnet-4.6", "gpt-5.5" -> "gpt-5.5"
 *  For 1M variants, strip the "-1m" suffix (the 1M badge handles display). */
function abbreviateModel(model: string): string {
  // Claude models: strip "claude-" prefix, strip "-1m" suffix, convert trailing version hyphen to dot
  if (!model.startsWith('claude-')) return model;
  let inner = model.slice(7); // strip "claude-"
  if (inner.endsWith('-1m')) {
    inner = inner.slice(0, -3);
  }
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
  onTriggerContinuation,
  onUpgradeModel,
  toolExecutingStartedAt,
}: StateBarProps) {
  const [pickerOpen, setPickerOpen] = useState(false);
  const [pickerShowAll, setPickerShowAll] = useState(false);
  // Mobile breakpoint mirrors the @media (max-width: 768px) block in index.css.
  const [isMobile, setIsMobile] = useState(() => window.matchMedia('(max-width: 768px)').matches);
  const [mobileExpanded, setMobileExpanded] = useState(false);
  const pickerRef = useRef<HTMLSpanElement>(null);

  // Live elapsed-time counter for tool_executing state.
  // Ticks every second; cleared immediately when leaving tool_executing.
  const [toolElapsedSeconds, setToolElapsedSeconds] = useState(0);
  useEffect(() => {
    if (convState.type !== 'tool_executing' || !toolExecutingStartedAt) {
      setToolElapsedSeconds(0);
      return;
    }
    // Compute immediately (avoids 1s lag on first render after transition)
    setToolElapsedSeconds(Math.floor((Date.now() - toolExecutingStartedAt) / 1000));
    const interval = window.setInterval(() => {
      setToolElapsedSeconds(Math.floor((Date.now() - toolExecutingStartedAt) / 1000));
    }, 1000);
    return () => window.clearInterval(interval);
  }, [convState.type, toolExecutingStartedAt]);
  useEffect(() => {
    const mq = window.matchMedia('(max-width: 768px)');
    const handler = (e: MediaQueryListEvent) => {
      setIsMobile(e.matches);
      if (!e.matches) setMobileExpanded(false);
    };
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, []);

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

  let dotClass = 'dot';
  let stateText = '';

  if (!conversation) {
    dotClass += ' hidden';
    stateText = '';
  } else {
    // Determine dot and text based on connection state first
    switch (connectionState) {
      case 'disconnected':
        dotClass += ' connecting';
        stateText = 'connecting...';
        break;

      case 'connecting':
        dotClass += ' connecting';
        stateText = 'connecting...';
        break;

      case 'reconnecting':
        dotClass += ' reconnecting';
        stateText = `reconnecting (${connectionAttempt})...`;
        break;

      case 'offline':
        dotClass += ' offline';
        stateText = 'offline';
        break;

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
            dotClass += ' terminal';
            stateText = 'completed';
            break;
          case 'awaiting_task_approval':
            dotClass += ' approval';
            stateText = 'awaiting approval';
            break;
          case 'awaiting_user_response':
            dotClass += ' approval';
            stateText = 'awaiting response';
            break;
          case 'error':
            dotClass += ' error';
            stateText = 'error';
            break;
          case 'context_exhausted':
            dotClass += ' error';
            stateText = 'context full';
            break;
          case 'awaiting_llm': case 'llm_requesting': case 'tool_executing':
          case 'awaiting_sub_agents': case 'awaiting_continuation':
          case 'cancelling': case 'cancelling_tool': case 'cancelling_sub_agents':
          case 'awaiting_recovery':
            dotClass += ' working';
            if (convState.type === 'tool_executing' && toolElapsedSeconds > 0) {
              stateText = `${getStateDescription(convState)} ... ${formatElapsed(toolElapsedSeconds)}`;
            } else {
              stateText = getStateDescription(convState);
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
  // Trigger menu only available in idle phase, regardless of threshold (the indicator gates on threshold itself).
  const indicatorTrigger = convState.type === 'idle' ? onTriggerContinuation : undefined;

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

  // Model picker: available on idle conversations when we have models and a callback.
  const currentModel = conversation?.model ?? '';
  const is1m = currentModel.endsWith('-1m');
  const canPickModel = !!(
    onUpgradeModel &&
    availableModels &&
    availableModels.length > 0 &&
    convState.type === 'idle'
  );

  // Shortcut: one-click upgrade to the 1M variant of the current model, if it
  // exists. The general picker also exposes 1M variants under "Show all", but
  // context-window upgrades are frequent enough to deserve a visible button.
  const upgradeTo1mId = is1m ? null : currentModel + '-1m';
  const canUpgradeTo1m = !!(
    upgradeTo1mId &&
    availableModels?.some(m => m.id === upgradeTo1mId) &&
    convState.type === 'idle' &&
    onUpgradeModel
  );

  const handleUpgradeTo1m = () => {
    if (!canUpgradeTo1m || !upgradeTo1mId || !onUpgradeModel) return;
    onUpgradeModel(upgradeTo1mId);
  };

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

  // Git delta badges
  const ahead = conversation?.commits_ahead;
  const behind = conversation?.commits_behind;
  const baseBranch = conversation?.base_branch;
  const branchName = conversation?.branch_name;
  const taskTitle = conversation?.task_title;

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
                      {is1m && <span className="model-1m-badge">1M</span>}
                      <span className="conv-model-caret" aria-hidden="true">&#9662;</span>
                    </button>
                  ) : (
                    <span className="conv-model" title={`Model: ${conversation.model ?? 'default'}`}>
                      {modelAbbrev}
                      {is1m && <span className="model-1m-badge">1M</span>}
                    </span>
                  )}
                  {canUpgradeTo1m && (
                    <button
                      className="model-upgrade-btn"
                      onClick={handleUpgradeTo1m}
                      title={`Upgrade to 1M context (${upgradeTo1mId})`}
                    >
                      1M
                    </button>
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
                    </span>
                  )}
                  {branchName && !baseBranch && (
                    <span className="git-branch-solo" title={`Branch: ${branchName}`}>
                      {branchName}
                    </span>
                  )}
                  {ahead != null && ahead > 0 && (
                    <span
                      className="git-badge git-badge--ahead"
                      title={`${ahead} commit(s) ahead of ${baseBranch || 'base'}`}
                    >
                      +{ahead}
                    </span>
                  )}
                  {behind != null && behind > 0 && (
                    <span
                      className="git-badge git-badge--behind"
                      title={`${behind} commit(s) behind ${baseBranch || 'base'} -- may need rebase`}
                    >
                      -{behind}
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
